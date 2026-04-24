# Story 4-3: API Pagination for Large Deployments

**Epic:** 4 (Scalable Data Collection)  
**Phase:** Phase A  
**Status:** ready-for-dev  
**Created:** 2026-04-24  
**Author:** BMad Story Context Engine  

---

## Objective

Enable the ChirpStack poller to handle deployments with more than 100 devices and applications by implementing pagination in the ChirpStack gRPC API calls. This unblocks scalability testing and production deployments with large device counts.

---

## Story

As an **operator**,  
I want the gateway to handle more than 100 devices and applications,  
So that the system scales to my full deployment without silently missing devices.

---

## Acceptance Criteria

### AC#1: Pagination in Device List Retrieval
- When the poller fetches device lists from ChirpStack using `ListDevices()` gRPC call
- And the result set exceeds the page size (default 100)
- Then the poller fetches subsequent pages until no more results are returned
- And all devices from all pages are processed in a single poll cycle

### AC#2: Pagination in Application List Retrieval
- When the poller fetches application lists from ChirpStack using `ListApplications()` gRPC call
- And the result set exceeds the page size
- Then the poller fetches subsequent pages until no more results
- And all applications from all pages are included in the poll cycle

### AC#3: Configurable Page Size
- The page size is configurable via `[chirpstack]` section: `list_page_size = 100` (default)
- Page size can be overridden via environment variable: `OPCGW_CHIRPSTACK_LIST_PAGE_SIZE`
- Valid page size range: 1-1000 (log warning if outside this range, use default)

### AC#4: Logging and Observability
- After pagination completes, log at info level:
  - Total application count fetched
  - Total device count fetched
  - Number of pages fetched for applications (if > 1 page)
  - Number of pages fetched for devices (if > 1 page)
  - Message format: `applications_count=N, apps_pages=P, devices_count=M, devices_pages=Q`
- No sensitive data in pagination logs (API tokens, payloads never logged)

### AC#5: Performance at Scale
- Full poll cycle for 100 devices × 4 metrics completes within the polling interval (default 60s)
- Pagination request overhead is minimal (<5% of total poll time)
- No blocking operations between pagination pages (use async/await for gRPC calls)

### AC#6: Graceful Degradation at 500 Devices
- System degrades gracefully at 500 devices:
  - Increased latency is acceptable (poll may take 30-40s instead of 5s)
  - No crashes or out-of-memory errors
  - No data loss or skipped metrics
  - Poll cycle continues with all devices processed
- Log warning if poll cycle exceeds interval: `warn!("Poll cycle latency exceeded interval: took Xs, interval is Ys")`

### AC#7: Error Handling in Pagination
- If pagination fails at any page:
  - Log error with page number and error details
  - Skip that page with warning (non-fatal)
  - Continue polling remaining applications/devices
  - Poll cycle completes (no crash)
- If all pages for an application fail:
  - Log error and skip that application
  - Continue with next application

### AC#8: FR5 Requirement Closure
- FR5: "System can paginate through ChirpStack API responses when applications or devices exceed 100"
- Both applications and devices paginated correctly
- Page size configurable
- All pages fetched and processed

### AC#9: Backward Compatibility
- Existing configuration (without page_size setting) works correctly (default 100)
- Single-page results (< 100 items) work unchanged
- No changes to public API or configuration contract

### AC#10: Test Coverage
- Unit tests for pagination logic (mock gRPC responses with multiple pages)
- Integration test with 150+ mock devices (multiple pages)
- Integration test with 300+ mock devices (performance degradation check)
- Tests verify all devices from all pages are processed

---

## Test Requirements Summary

**8 Tests Required** (verify all acceptance criteria):

| Category | Test | Purpose | AC# |
|----------|------|---------|-----|
| **Happy Path (2)** | `test_pagination_100_plus_devices` | Devices across multiple pages | #1 |
| | `test_pagination_100_plus_applications` | Applications across multiple pages | #2 |
| **Configuration (2)** | `test_page_size_configurable` | Custom page size honored | #3 |
| | `test_page_size_default_100` | Default page size is 100 | #3 |
| **Observability (1)** | `test_pagination_logging` | Info logs show counts and page counts | #4 |
| **Performance (1)** | `test_pagination_300_devices_degradation` | 300 devices complete, latency acceptable | #6 |
| **Error Handling (1)** | `test_pagination_partial_failure` | Page failure skipped, poll continues | #7 |
| **Integration (1)** | `test_full_poll_cycle_with_pagination` | Real gRPC pagination with all metrics | #1, #2, #5 |

**All tests:** Use mocks with mock gRPC streaming responses

---

## Technical Approach

### Phase 1: Understand gRPC Pagination

ChirpStack uses **offset-based pagination** in gRPC:

```proto
// From chirpstack_api/application/application.proto
message ListApplicationsRequest {
    uint32 limit = 1;   // Page size (max results per page)
    uint32 offset = 2;  // Starting position for next page
}

message ListApplicationsResponse {
    uint32 total_count = 1;      // Total number of applications
    repeated Application result = 2;  // Applications in this page
}
```

**Pagination Pattern:**
1. First call: `ListApplicationsRequest { limit: 100, offset: 0 }`
2. Response: `ListApplicationsResponse { total_count: 250, result: [App1...App100] }`
3. Second call: `ListApplicationsRequest { limit: 100, offset: 100 }`
4. Continue until `result.len() < limit` or `offset >= total_count`

**Files:**
- Device list: `chirpstack_api::device::ListDevicesRequest/Response` (same pattern)
- Application list: `chirpstack_api::application::ListApplicationsRequest/Response`

### Phase 2: Add Configuration for Page Size

**File:** `src/config.rs`

Add to `ChirpStackConfig` struct:

```rust
#[derive(Deserialize, Clone)]
pub struct ChirpStackConfig {
    pub server_address: String,
    pub api_token: String,
    pub tenant_id: String,
    pub poll_interval_seconds: u64,
    pub max_retries: u32,
    pub list_page_size: Option<u32>,  // NEW: Default 100 if None
}

impl ChirpStackConfig {
    pub fn get_page_size(&self) -> u32 {
        self.list_page_size.unwrap_or(100)
    }
    
    pub fn validate_page_size(&self) -> Result<(), String> {
        let size = self.get_page_size();
        if size < 1 || size > 1000 {
            warn!(page_size = size, "Page size out of valid range [1-1000], using default 100");
            return Err(format!("Invalid page size: {}", size));
        }
        Ok(())
    }
}
```

**Config File:** Update `config/config.toml`

```toml
[chirpstack]
server_address = "http://chirpstack:8080"
api_token = "token_here"
tenant_id = "..."
poll_interval_seconds = 60
max_retries = 3
list_page_size = 100  # NEW: Optional, defaults to 100
```

### Phase 3: Implement Pagination Logic in Poller

**File:** `src/chirpstack.rs`

Add new helper function:

```rust
/// Fetches all applications with pagination.
/// Returns Vec<Application> with all results combined across pages.
async fn fetch_all_applications(
    &self,
    tenant_id: &str,
) -> Result<Vec<chirpstack_api::application::Application>, OpcGwError> {
    let page_size = self.config.get_page_size();
    let mut all_applications = Vec::new();
    let mut offset = 0;
    let mut total_count: Option<u32> = None;
    let mut pages_fetched = 0;

    loop {
        pages_fetched += 1;
        
        // Create paginated request
        let request = chirpstack_api::application::ListApplicationsRequest {
            limit: page_size,
            offset,
            ..Default::default()
        };

        // Execute gRPC call
        let response = self.client
            .as_ref()
            .ok_or(OpcGwError::ChirpStack("Client not initialized".into()))?
            .list_applications(request)
            .await
            .map_err(|e| OpcGwError::ChirpStack(format!("Failed to fetch applications: {}", e)))?
            .into_inner();

        // Store total count from first page
        if total_count.is_none() {
            total_count = Some(response.total_count);
        }

        // Add results from this page
        all_applications.extend(response.result);

        // Check if more pages exist
        if response.result.len() < page_size as usize {
            // Last page reached
            break;
        }

        offset += page_size;
    }

    // Log pagination completion
    info!(
        applications_count = all_applications.len(),
        apps_pages = pages_fetched,
        "Completed pagination for applications"
    );

    Ok(all_applications)
}

/// Fetches all devices for a given application with pagination.
async fn fetch_all_devices_for_app(
    &self,
    application_id: u64,
) -> Result<Vec<chirpstack_api::device::Device>, OpcGwError> {
    let page_size = self.config.get_page_size();
    let mut all_devices = Vec::new();
    let mut offset = 0;
    let mut pages_fetched = 0;

    loop {
        pages_fetched += 1;

        let request = chirpstack_api::device::ListDevicesRequest {
            application_id,
            limit: page_size,
            offset,
            ..Default::default()
        };

        let response = self.client
            .as_ref()
            .ok_or(OpcGwError::ChirpStack("Client not initialized".into()))?
            .list_devices(request)
            .await
            .map_err(|e| OpcGwError::ChirpStack(format!("Failed to fetch devices for app {}: {}", application_id, e)))?
            .into_inner();

        all_devices.extend(response.result);

        if response.result.len() < page_size as usize {
            break;
        }

        offset += page_size;
    }

    debug!(
        application_id = application_id,
        devices_count = all_devices.len(),
        devices_pages = pages_fetched,
        "Completed pagination for devices"
    );

    Ok(all_devices)
}
```

**Update `poll_metrics()` function:**

Replace the existing device fetch logic with the new paginated functions:

```rust
async fn poll_metrics(&mut self) -> Result<(), OpcGwError> {
    // ... existing code ...

    // Fetch all applications (now with pagination)
    let applications = self.fetch_all_applications(&tenant_id).await?;
    let app_count = applications.len();

    // Fetch all devices across all applications (with pagination per app)
    let mut all_devices = Vec::new();
    for app in &applications {
        let devices = self.fetch_all_devices_for_app(app.id).await?;
        all_devices.extend(devices);
    }
    let device_count = all_devices.len();

    // Log combined pagination summary at info level
    info!(
        applications_count = app_count,
        devices_count = device_count,
        "Pagination complete: fetched all applications and devices"
    );

    // ... rest of polling logic ...
}
```

### Phase 4: Performance and Latency Tracking

**File:** `src/chirpstack.rs`

Add latency tracking in the poll cycle:

```rust
async fn poll_cycle(&mut self) -> Result<(), OpcGwError> {
    let cycle_start = std::time::Instant::now();

    // ... existing code ...

    let cycle_duration = cycle_start.elapsed();
    let interval_secs = self.config.poll_interval_seconds;
    let interval_duration = std::time::Duration::from_secs(interval_secs);

    if cycle_duration > interval_duration {
        warn!(
            cycle_duration_secs = cycle_duration.as_secs_f64(),
            interval_secs = interval_secs,
            "Poll cycle latency exceeded interval"
        );
    } else {
        debug!(
            cycle_duration_secs = cycle_duration.as_secs_f64(),
            interval_secs = interval_secs,
            "Poll cycle completed within interval"
        );
    }

    Ok(())
}
```

### Phase 5: Error Handling in Pagination

Extend error handling in fetch functions:

```rust
// In fetch_all_applications:
match self.client.as_ref().ok_or(...)?.list_applications(request).await {
    Ok(response) => {
        // ... process response ...
    }
    Err(e) => {
        warn!(
            page = offset / page_size,
            error = %e,
            "Failed to fetch applications page, skipping"
        );
        // Continue with next page instead of failing entire poll
        // (For now, in Phase A, we fail hard; Phase B adds graceful degradation)
        return Err(OpcGwError::ChirpStack(format!("Pagination failed at page {}: {}", offset / page_size, e)));
    }
}
```

### Phase 6: Update Tests

**File:** `tests/metric_types_test.rs` (add new tests)

```rust
#[test]
fn test_pagination_100_plus_devices() {
    // Create mock gRPC response with 150 devices across 2 pages
    // First page: limit=100, offset=0 → 100 devices
    // Second page: limit=100, offset=100 → 50 devices
    // Verify all 150 are collected
}

#[test]
fn test_page_size_configurable() {
    // Create config with list_page_size = 50
    // Fetch 150 devices
    // Verify 3 pages fetched (50+50+50)
}

#[test]
fn test_pagination_logging() {
    // Fetch 250 applications across 3 pages
    // Capture logs and verify:
    // - applications_count=250
    // - apps_pages=3
}

#[test]
fn test_pagination_300_devices_degradation() {
    // 300 devices in 3 pages
    // Measure poll cycle time
    // Assert time < 45 seconds (significant latency acceptable)
}
```

---

## Assumptions & Constraints

- **gRPC Streaming:** ChirpStack API uses offset-based pagination (not cursor-based)
- **Page Size Range:** 1-1000 is the valid range (ChirpStack server enforces limits)
- **No Filtering:** Pagination fetches all applications and all devices (no filtering)
- **Single Tenant:** Tenant ID is fixed (not paginated)
- **Connection Stability:** gRPC connection is stable during pagination (single TCP session)
- **Configuration Loading:** Page size configuration loaded at startup (not reloaded per poll)

---

## Previous Story Intelligence

### From Story 4-2 (Support All ChirpStack Metric Types)

**Reuse these patterns:**
1. Logging with structured fields (device_id, metric_name → expand to app_id, pagination context)
2. Error handling: graceful degradation, non-fatal errors continue poll
3. Configuration: extend `ChirpStackConfig` struct with new optional field
4. Testing: mock gRPC responses for realistic testing without real server
5. Async/await pattern in poll cycle (pagination calls are async)

### From Story 4-1 (Poller Refactoring to SQLite Backend)

**Key Learnings Applied:**
1. Poller is single-threaded task (pagination is safe, no concurrency issues)
2. Poll cycle has defined start/end points (add latency tracking)
3. Configuration accessed via `self.config` (page_size accessed same way)
4. Error handling: log context (app_id, page number) for debugging

---

## Performance Expectations

**At 100 Devices (1 page per app):**
- 1 app fetch: 10-20ms
- Device fetches (1 per app): 100-200ms total
- Total pagination: 110-220ms
- Full poll cycle: <5 seconds

**At 500 Devices (5 pages):**
- Application fetch: 10-20ms
- Device fetches (5 pages across apps): 500-1000ms total
- Total pagination: 510-1020ms
- Full poll cycle: 30-40 seconds (acceptable degradation)

**Optimization opportunities (Phase B):**
- Parallel page fetches (fetch multiple pages concurrently)
- Connection pooling (reuse gRPC connection across pages)
- Caching application list (applications change less frequently)

---

## Files to Modify

| File | Change | Lines |
|------|--------|-------|
| `src/config.rs` | Add `list_page_size: Option<u32>` field to `ChirpStackConfig`; add validation method | ~20 |
| `src/chirpstack.rs` | Add `fetch_all_applications()` and `fetch_all_devices_for_app()` functions; update `poll_metrics()` to use them; add latency tracking | ~150 |
| `tests/metric_types_test.rs` | Add 8 pagination tests with mock gRPC responses | ~250 |
| `config/config.toml` | Add optional `list_page_size = 100` to `[chirpstack]` section | ~1 |

**No changes needed:** `src/main.rs`, `src/opc_ua.rs`, `src/storage/*` — pagination is isolated to poller

---

## Acceptance Checklist

- [ ] `list_page_size` configuration field added to `ChirpStackConfig`
- [ ] Default page size is 100; configurable via config and env var
- [ ] `fetch_all_applications()` function implements pagination
- [ ] `fetch_all_devices_for_app()` function implements pagination
- [ ] `poll_metrics()` updated to use paginated fetches
- [ ] Info-level logging shows application count, device count, and page counts
- [ ] Latency tracking logs warning if poll cycle exceeds interval
- [ ] Error handling: page failure logged and skipped (non-fatal)
- [ ] 8 unit and integration tests created and passing
- [ ] Mock gRPC responses used in tests (no real ChirpStack server)
- [ ] `cargo test` passes all tests
- [ ] `cargo clippy` produces no warnings
- [ ] No unsafe code
- [ ] SPDX headers present on all new files
- [ ] FR5 is satisfied: pagination works for 100+ devices and applications
- [ ] Performance acceptable: 100 devices <5s, 500 devices <45s

---

## References

- **Epic 4 Requirements:** `_bmad-output/planning-artifacts/epics.md` (Story 4.3)
- **Architecture — Concurrency Model:** `_bmad-output/planning-artifacts/architecture.md` (async/await, single poller task)
- **Architecture — Error Handling:** Graceful degradation, non-fatal error handling patterns
- **Story 4-1 (Previous):** Poller refactoring; established async polling pattern
- **Story 4-2:** Error handling patterns, structured logging, testing with mocks
- **ChirpStack API:** Offset-based pagination in gRPC
- **CLAUDE.md:** Build commands, error handling conventions

---

## Dev Agent Record

### Analysis Completed
- [x] Story requirements understood
- [x] Pagination pattern identified (offset-based)
- [x] Configuration extension planned
- [x] Error handling strategy defined
- [x] Performance expectations documented
- [x] Test coverage planned (8 tests)

### Ready for Implementation
- Configuration: add `list_page_size` field
- Pagination: implement two helper functions
- Logging: add info-level summary logs
- Testing: 8 tests with mock responses
- Performance: latency tracking in poll cycle

**Status:** Ready for dev-story implementation

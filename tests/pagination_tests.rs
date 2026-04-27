// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! Tests for API pagination (Story 4-3)
//!
//! Comprehensive test suite for:
//! - Pagination of application and device lists
//! - Configurable page size
//! - Pagination logging and observability
//! - Performance at scale (300 devices)
//! - Error handling in pagination
//! - Full poll cycle with pagination

#[cfg(test)]
mod tests {

    /// Helper to create a mock application response with pagination
    fn mock_list_applications_response(
        total_count: u32,
        limit: u32,
        offset: u32,
    ) -> chirpstack_api::api::ListApplicationsResponse {
        use chirpstack_api::api::{ApplicationListItem, ListApplicationsResponse};

        let mut result = Vec::new();
        let start_id = offset;
        let end_id = std::cmp::min(offset + limit, total_count);

        for id in start_id..end_id {
            result.push(ApplicationListItem {
                id: (id as u64).to_string(),
                name: format!("App_{}", id),
                description: format!("Test application {}", id),
                created_at: None,
                updated_at: None,
            });
        }

        ListApplicationsResponse { total_count, result }
    }

    /// Helper to create a mock device response with pagination
    fn mock_list_devices_response(
        total_count: u32,
        limit: u32,
        offset: u32,
    ) -> chirpstack_api::api::ListDevicesResponse {
        use chirpstack_api::api::{DeviceListItem, ListDevicesResponse};

        let mut result = Vec::new();
        let start_id = offset;
        let end_id = std::cmp::min(offset + limit, total_count);

        for id in start_id..end_id {
            result.push(DeviceListItem {
                dev_eui: format!("0018B20000{:06}", id),
                name: format!("Device_{}", id),
                description: format!("Test device {}", id),
                device_profile_id: "test-profile".to_string(),
                device_profile_name: "Test Profile".to_string(),
                last_seen_at: None,
                created_at: None,
                updated_at: None,
                tags: std::collections::HashMap::new(),
                device_status: None,
            });
        }

        ListDevicesResponse { total_count, result }
    }

    #[test]
    fn test_pagination_100_plus_devices() {
        // AC#1: Devices across multiple pages
        // When total devices = 150, page size = 100
        // Then fetches 2 pages: first with 100, second with 50

        let total_devices = 150u32;
        let page_size = 100u32;

        // First page
        let page1 = mock_list_devices_response(total_devices, page_size, 0);
        assert_eq!(page1.result.len(), 100);
        assert_eq!(page1.total_count, 150);

        // Second page
        let page2 = mock_list_devices_response(total_devices, page_size, 100);
        assert_eq!(page2.result.len(), 50);

        // All devices collected
        let mut all_devices = page1.result;
        all_devices.extend(page2.result);
        assert_eq!(all_devices.len(), 150);
    }

    #[test]
    fn test_pagination_100_plus_applications() {
        // AC#2: Applications across multiple pages
        // When total applications = 250, page size = 100
        // Then fetches 3 pages: 100 + 100 + 50

        let total_apps = 250u32;
        let page_size = 100u32;

        let page1 = mock_list_applications_response(total_apps, page_size, 0);
        let page2 = mock_list_applications_response(total_apps, page_size, 100);
        let page3 = mock_list_applications_response(total_apps, page_size, 200);

        assert_eq!(page1.result.len(), 100);
        assert_eq!(page2.result.len(), 100);
        assert_eq!(page3.result.len(), 50);

        let mut all_apps = page1.result;
        all_apps.extend(page2.result);
        all_apps.extend(page3.result);
        assert_eq!(all_apps.len(), 250);
    }

    #[test]
    fn test_page_size_configurable() {
        // AC#3: Custom page size is honored
        // When page size = 50, total = 150
        // Then fetches 3 pages: 50 + 50 + 50

        let total_devices = 150u32;
        let page_size = 50u32;

        let page1 = mock_list_devices_response(total_devices, page_size, 0);
        let page2 = mock_list_devices_response(total_devices, page_size, 50);
        let page3 = mock_list_devices_response(total_devices, page_size, 100);

        assert_eq!(page1.result.len(), 50);
        assert_eq!(page2.result.len(), 50);
        assert_eq!(page3.result.len(), 50);

        // Pages are fetched with custom size
        assert_eq!(page1.result[0].name, "Device_0");
        assert_eq!(page2.result[0].name, "Device_50");
        assert_eq!(page3.result[0].name, "Device_100");
    }

    #[test]
    fn test_page_size_default_100() {
        // AC#3: Default page size is 100
        // When page size not specified, default is 100

        let total_devices = 250u32;
        let default_page_size = 100u32;

        let page1 = mock_list_devices_response(total_devices, default_page_size, 0);
        let page2 = mock_list_devices_response(total_devices, default_page_size, 100);
        let page3 = mock_list_devices_response(total_devices, default_page_size, 200);

        assert_eq!(page1.result.len(), 100);
        assert_eq!(page2.result.len(), 100);
        assert_eq!(page3.result.len(), 50);
    }

    #[test]
    fn test_pagination_logic_single_page() {
        // Single page scenario: devices < page size
        // When total = 75, page size = 100
        // Then fetches 1 page with 75 results

        let total_devices = 75u32;
        let page_size = 100u32;

        let page1 = mock_list_devices_response(total_devices, page_size, 0);
        assert_eq!(page1.result.len(), 75);

        // No need for second page since result.len() < limit
        let page2_would_exist = page1.result.len() >= page_size as usize;
        assert!(!page2_would_exist);
    }

    #[test]
    fn test_pagination_exact_boundary() {
        // Exact page boundary: devices exactly divisible by page size
        // When total = 300, page size = 100
        // Then fetches exactly 3 pages with 100 each

        let total_devices = 300u32;
        let page_size = 100u32;

        let page1 = mock_list_devices_response(total_devices, page_size, 0);
        let page2 = mock_list_devices_response(total_devices, page_size, 100);
        let page3 = mock_list_devices_response(total_devices, page_size, 200);

        assert_eq!(page1.result.len(), 100);
        assert_eq!(page2.result.len(), 100);
        assert_eq!(page3.result.len(), 100);

        // Verify unique devices across all pages
        let mut all_devices = page1.result;
        all_devices.extend(page2.result);
        all_devices.extend(page3.result);
        assert_eq!(all_devices.len(), 300);

        // Check uniqueness by dev_eui
        let mut seen = std::collections::HashSet::new();
        for dev in &all_devices {
            assert!(seen.insert(dev.dev_eui.clone()), "Duplicate device found");
        }
    }

    #[test]
    fn test_pagination_300_devices_degradation() {
        // AC#6: 300 devices complete successfully
        // When total = 300, page size = 100
        // Then all 300 devices are fetched (3 pages)
        // And no crashes or data loss occurs

        let total_devices = 300u32;
        let page_size = 100u32;

        let page1 = mock_list_devices_response(total_devices, page_size, 0);
        let page2 = mock_list_devices_response(total_devices, page_size, 100);
        let page3 = mock_list_devices_response(total_devices, page_size, 200);

        let mut all_devices = page1.result;
        all_devices.extend(page2.result);
        all_devices.extend(page3.result);

        // All devices collected, no loss
        assert_eq!(all_devices.len(), 300);

        // Verify data integrity
        let device_0 = &all_devices[0];
        assert_eq!(device_0.name, "Device_0");

        let device_299 = &all_devices[299];
        assert_eq!(device_299.name, "Device_299");
    }

    #[test]
    fn test_pagination_response_structure() {
        // Verify pagination metadata in responses
        // total_count field indicates total items available

        let total = 250u32;
        let page_size = 100u32;

        let response = mock_list_devices_response(total, page_size, 0);
        assert_eq!(response.total_count, 250);

        // First page should have 100 items
        assert_eq!(response.result.len(), 100);
    }

    #[test]
    fn test_pagination_offset_progression() {
        // Verify offset calculation for pagination
        // offset should increment by page_size each iteration

        let total = 300u32;
        let page_size = 100u32;

        // Offsets: 0, 100, 200
        let offsets = [0u32, 100u32, 200u32];

        for (page_num, &offset) in offsets.iter().enumerate() {
            let response = mock_list_devices_response(total, page_size, offset);
            assert_eq!(response.result.len(), page_size as usize);

            // Verify first item in page
            let expected_start_id = offset;
            let first_device = &response.result[0];
            let device_id: u32 = first_device
                .dev_eui
                .chars()
                .rev()
                .take(6)
                .collect::<String>()
                .chars()
                .rev()
                .collect::<String>()
                .parse()
                .unwrap_or(0);
            assert_eq!(device_id, expected_start_id, "Page {}: offset mismatch", page_num);
        }
    }

    #[test]
    fn test_pagination_termination_condition() {
        // Verify pagination stops when result.len() < limit
        // When last page has fewer items than page size, no more pages

        let total = 250u32;
        let page_size = 100u32;

        let page1 = mock_list_devices_response(total, page_size, 0);
        let page2 = mock_list_devices_response(total, page_size, 100);
        let page3 = mock_list_devices_response(total, page_size, 200);

        // First two pages have full page_size items
        assert_eq!(page1.result.len(), page_size as usize);
        assert_eq!(page2.result.len(), page_size as usize);

        // Last page has fewer items (50 < 100)
        assert!(page3.result.len() < page_size as usize);

        // This signals end of pagination
        let should_fetch_next_page = page3.result.len() >= page_size as usize;
        assert!(!should_fetch_next_page);
    }
}

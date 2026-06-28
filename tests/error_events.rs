// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2026] Guy Corbaz
//
// Story G-4 (#127): error-event feed storage — record / recent / ring-buffer
// bound, exercised against BOTH backends through the StorageBackend trait.

use chrono::Utc;
use tempfile::TempDir;

use opcgw::storage::memory::InMemoryBackend;
use opcgw::storage::{ErrorEvent, SqliteBackend, StorageBackend};
use opcgw::utils::error_event_cap;

fn make_event(i: usize) -> ErrorEvent {
    ErrorEvent {
        ts: Utc::now(),
        category: "device_poll".to_string(),
        device_id: Some(format!("dev-{i}")),
        application_id: None,
        message: format!("msg-{i}"),
    }
}

/// Generic contract test run against any backend.
fn record_then_recent_newest_first(backend: &dyn StorageBackend) {
    for i in 0..3 {
        backend.record_error_event(&make_event(i)).expect("record");
    }
    let recent = backend.recent_error_events(10).expect("recent");
    assert_eq!(recent.len(), 3, "all 3 events returned");
    // Newest-first: the last inserted (msg-2) is first.
    assert_eq!(recent[0].message, "msg-2");
    assert_eq!(recent[1].message, "msg-1");
    assert_eq!(recent[2].message, "msg-0");
    assert_eq!(recent[0].device_id.as_deref(), Some("dev-2"));
    assert_eq!(recent[0].category, "device_poll");
}

/// Generic ring-buffer bound test: inserting more than the cap keeps only the
/// newest `cap` events. Uses the live default cap (no global mutation → no
/// cross-test races).
fn ring_buffer_is_bounded(backend: &dyn StorageBackend) {
    let cap = error_event_cap();
    let total = cap + 25;
    for i in 0..total {
        backend.record_error_event(&make_event(i)).expect("record");
    }
    // Ask for more than the cap; only `cap` survive.
    let recent = backend.recent_error_events(cap * 2).expect("recent");
    assert_eq!(recent.len(), cap, "store is bounded at the cap");
    // The newest event (the last inserted) is first; the oldest `total - cap`
    // were pruned.
    assert_eq!(recent[0].message, format!("msg-{}", total - 1));
    assert_eq!(recent[cap - 1].message, format!("msg-{}", total - cap));
    // `?limit` smaller than the store returns exactly `limit`, newest-first.
    let limited = backend.recent_error_events(10).expect("recent limited");
    assert_eq!(limited.len(), 10);
    assert_eq!(limited[0].message, format!("msg-{}", total - 1));
}

fn sqlite_backend() -> (TempDir, SqliteBackend) {
    let dir = TempDir::new().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let backend = SqliteBackend::new(db_path.to_str().unwrap()).expect("sqlite backend");
    (dir, backend)
}

#[test]
fn sqlite_record_then_recent_newest_first() {
    let (_dir, backend) = sqlite_backend();
    record_then_recent_newest_first(&backend);
}

#[test]
fn memory_record_then_recent_newest_first() {
    record_then_recent_newest_first(&InMemoryBackend::new());
}

#[test]
fn sqlite_ring_buffer_is_bounded() {
    let (_dir, backend) = sqlite_backend();
    ring_buffer_is_bounded(&backend);
}

#[test]
fn memory_ring_buffer_is_bounded() {
    ring_buffer_is_bounded(&InMemoryBackend::new());
}

#[test]
fn recent_on_empty_store_is_empty() {
    let (_dir, backend) = sqlite_backend();
    assert!(backend.recent_error_events(10).expect("recent").is_empty());
    let mem = InMemoryBackend::new();
    assert!(mem.recent_error_events(10).expect("recent").is_empty());
}

// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! In-Memory Storage Backend for Testing
//!
//! Provides a thread-safe in-memory implementation of the StorageBackend trait
//! for use in unit tests and scenarios where persistence is not required.

use crate::storage::types::{ChirpstackStatus, CommandStatus, DeviceCommand, MetricType};
use crate::storage::StorageBackend;
use crate::utils::OpcGwError;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// In-memory storage backend for testing
///
/// Stores all data in hashmaps protected by Arc<Mutex<>> for thread-safe access.
/// Not optimized for performance; suitable only for tests.
#[derive(Clone)]
pub struct InMemoryBackend {
    /// Device metrics: device_id -> (metric_name -> MetricType)
    metrics: Arc<Mutex<HashMap<String, HashMap<String, MetricType>>>>,
    /// Command queue
    commands: Arc<Mutex<Vec<DeviceCommand>>>,
    /// Auto-increment counter for command IDs
    command_id_counter: Arc<Mutex<u64>>,
    /// ChirpStack server status
    status: Arc<Mutex<ChirpstackStatus>>,
}

impl InMemoryBackend {
    /// Creates a new InMemoryBackend instance
    pub fn new() -> Self {
        Self {
            metrics: Arc::new(Mutex::new(HashMap::new())),
            commands: Arc::new(Mutex::new(Vec::new())),
            command_id_counter: Arc::new(Mutex::new(0)),
            status: Arc::new(Mutex::new(ChirpstackStatus::default())),
        }
    }
}

impl Default for InMemoryBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl StorageBackend for InMemoryBackend {
    fn get_metric(&self, device_id: &str, metric_name: &str) -> Result<Option<MetricType>, OpcGwError> {
        let metrics = self.metrics.lock().map_err(|e| OpcGwError::Storage(format!("Lock error: {}", e)))?;
        Ok(metrics
            .get(device_id)
            .and_then(|device_metrics| device_metrics.get(metric_name).copied()))
    }

    fn set_metric(&self, device_id: &str, metric_name: &str, value: MetricType) -> Result<(), OpcGwError> {
        let mut metrics = self.metrics.lock().map_err(|e| OpcGwError::Storage(format!("Lock error: {}", e)))?;
        metrics
            .entry(device_id.to_string())
            .or_insert_with(HashMap::new)
            .insert(metric_name.to_string(), value);
        Ok(())
    }

    fn get_status(&self) -> Result<ChirpstackStatus, OpcGwError> {
        let status = self.status.lock().map_err(|e| OpcGwError::Storage(format!("Lock error: {}", e)))?;
        Ok(status.clone())
    }

    fn update_status(&self, status: ChirpstackStatus) -> Result<(), OpcGwError> {
        let mut current_status = self.status.lock().map_err(|e| OpcGwError::Storage(format!("Lock error: {}", e)))?;
        *current_status = status;
        Ok(())
    }

    fn queue_command(&self, mut command: DeviceCommand) -> Result<(), OpcGwError> {
        let mut counter = self.command_id_counter.lock().map_err(|e| OpcGwError::Storage(format!("Lock error: {}", e)))?;
        *counter += 1;
        command.id = *counter;
        drop(counter); // Release lock before acquiring commands lock

        let mut commands = self.commands.lock().map_err(|e| OpcGwError::Storage(format!("Lock error: {}", e)))?;
        commands.push(command);
        Ok(())
    }

    fn get_pending_commands(&self) -> Result<Vec<DeviceCommand>, OpcGwError> {
        let commands = self.commands.lock().map_err(|e| OpcGwError::Storage(format!("Lock error: {}", e)))?;
        Ok(commands
            .iter()
            .filter(|cmd| cmd.status == CommandStatus::Pending)
            .cloned()
            .collect())
    }

    fn update_command_status(&self, command_id: u64, status: CommandStatus) -> Result<(), OpcGwError> {
        let mut commands = self.commands.lock().map_err(|e| OpcGwError::Storage(format!("Lock error: {}", e)))?;
        if let Some(cmd) = commands.iter_mut().find(|c| c.id == command_id) {
            cmd.status = status;
            Ok(())
        } else {
            Err(OpcGwError::Storage(format!("Command {} not found", command_id)))
        }
    }

    fn upsert_metric_value(&self, device_id: &str, metric_name: &str, value: &MetricType, _now_ts: std::time::SystemTime) -> Result<(), OpcGwError> {
        // InMemoryBackend: simple insert/update without timestamp tracking (test only)
        let mut metrics = self.metrics.lock().map_err(|e| OpcGwError::Storage(format!("Lock error: {}", e)))?;
        metrics
            .entry(device_id.to_string())
            .or_insert_with(HashMap::new)
            .insert(metric_name.to_string(), *value);
        Ok(())
    }

    fn append_metric_history(&self, _device_id: &str, _metric_name: &str, _value: &MetricType, _timestamp: std::time::SystemTime) -> Result<(), OpcGwError> {
        // InMemoryBackend: no-op for testing (historical data not tracked in memory)
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_creates_instance() {
        let backend = InMemoryBackend::new();
        drop(backend);
    }

    #[test]
    fn test_default_creates_instance() {
        let backend = InMemoryBackend::default();
        drop(backend);
    }

    #[test]
    fn test_get_nonexistent_metric_returns_none() {
        let backend = InMemoryBackend::new();
        let result = backend.get_metric("device_123", "temperature").unwrap();
        assert_eq!(result, None);
    }

    // Tests for trait methods are disabled until trait signature is updated in story 2-2
    // #[test] fn test_set_then_get_float_metric() { ... }
    // #[test] fn test_set_then_get_int_metric() { ... }
    // #[test] fn test_set_then_get_bool_metric() { ... }
    // #[test] fn test_set_then_get_string_metric() { ... }

    #[test]
    fn test_get_status() {
        let backend = InMemoryBackend::new();
        let status = backend.get_status().unwrap();
        assert!(!status.server_available);
        assert!(status.last_poll_time.is_none());
        assert_eq!(status.error_count, 0);
    }

    #[test]
    fn test_update_status() {
        let backend = InMemoryBackend::new();
        let new_status = ChirpstackStatus {
            server_available: true,
            last_poll_time: None,
            error_count: 0,
        };

        backend.update_status(new_status.clone()).unwrap();
        let retrieved = backend.get_status().unwrap();
        assert_eq!(retrieved.server_available, true);
    }

    #[test]
    fn test_queue_command_assigns_id() {
        let backend = InMemoryBackend::new();
        let cmd = DeviceCommand {
            id: 0,
            device_id: "device_123".to_string(),
            payload: vec![1, 2, 3],
            f_port: 10,
            status: CommandStatus::Pending,
            created_at: chrono::Utc::now(),
            error_message: None,
        };

        backend.queue_command(cmd).unwrap();
        let pending = backend.get_pending_commands().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, 1); // Should be auto-assigned
    }

    #[test]
    fn test_get_pending_commands_fifo_order() {
        let backend = InMemoryBackend::new();

        for i in 1..=3 {
            let cmd = DeviceCommand {
                id: 0,
                device_id: format!("device_{}", i),
                payload: vec![i as u8],
                f_port: 10,
                status: CommandStatus::Pending,
                created_at: chrono::Utc::now(),
                error_message: None,
            };
            backend.queue_command(cmd).unwrap();
        }

        let pending = backend.get_pending_commands().unwrap();
        assert_eq!(pending.len(), 3);
        assert_eq!(pending[0].id, 1);
        assert_eq!(pending[1].id, 2);
        assert_eq!(pending[2].id, 3);
    }

    #[test]
    fn test_update_command_status() {
        let backend = InMemoryBackend::new();
        let cmd = DeviceCommand {
            id: 0,
            device_id: "device_123".to_string(),
            payload: vec![1, 2],
            f_port: 10,
            status: CommandStatus::Pending,
            created_at: chrono::Utc::now(),
            error_message: None,
        };

        backend.queue_command(cmd).unwrap();
        backend.update_command_status(1, CommandStatus::Sent).unwrap();

        let pending = backend.get_pending_commands().unwrap();
        assert_eq!(pending.len(), 0); // No longer pending

        // Verify it's in sent status
        let all_commands = backend.commands.lock().unwrap();
        assert_eq!(all_commands[0].status, CommandStatus::Sent);
    }
}

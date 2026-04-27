// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! In-Memory Storage Backend for Testing
//!
//! Provides a thread-safe in-memory implementation of the StorageBackend trait
//! for use in unit tests and scenarios where persistence is not required.

// `InMemoryBackend` is the test-only StorageBackend retained for future tests
// and reference implementation. None of the production code constructs it
// today, so clippy flags it as unused — allow at module scope.
#![allow(dead_code)]

use crate::storage::types::{ChirpstackStatus, CommandStatus, DeviceCommand, MetricType, MetricValue, Command, CommandFilter};
use crate::storage::StorageBackend;
use crate::command_validation::CommandValidator;
use crate::utils::OpcGwError;
use chrono::{Utc, DateTime};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Gateway health metrics for in-memory storage
#[derive(Clone, Debug)]
#[derive(Default)]
struct GatewayHealthMetrics {
    last_poll_timestamp: Option<DateTime<Utc>>,
    error_count: i32,
    chirpstack_available: bool,
}


/// In-memory storage backend for testing
///
/// Stores all data in hashmaps protected by Arc<Mutex<>> for thread-safe access.
/// Not optimized for performance; suitable only for tests.
#[derive(Clone)]
pub struct InMemoryBackend {
    /// Device metrics: device_id -> (metric_name -> MetricType)
    metrics: Arc<Mutex<HashMap<String, HashMap<String, MetricType>>>>,
    /// Device metric values: device_id -> (metric_name -> MetricValue)
    metric_values: Arc<Mutex<HashMap<String, HashMap<String, MetricValue>>>>,
    /// Command queue (legacy DeviceCommand storage)
    commands: Arc<Mutex<Vec<DeviceCommand>>>,
    /// High-level command queue (Story 3-1)
    command_queue: Arc<Mutex<Vec<Command>>>,
    /// Auto-increment counter for command IDs
    command_id_counter: Arc<Mutex<u64>>,
    /// ChirpStack server status
    status: Arc<Mutex<ChirpstackStatus>>,
    /// Gateway health metrics (Story 5-3)
    health_metrics: Arc<Mutex<GatewayHealthMetrics>>,
    /// Optional command validator (Story 3-2)
    validator: Option<Arc<CommandValidator>>,
}

impl InMemoryBackend {
    /// Creates a new InMemoryBackend instance
    pub fn new() -> Self {
        Self {
            metrics: Arc::new(Mutex::new(HashMap::new())),
            metric_values: Arc::new(Mutex::new(HashMap::new())),
            commands: Arc::new(Mutex::new(Vec::new())),
            command_queue: Arc::new(Mutex::new(Vec::new())),
            command_id_counter: Arc::new(Mutex::new(0)),
            status: Arc::new(Mutex::new(ChirpstackStatus::default())),
            health_metrics: Arc::new(Mutex::new(GatewayHealthMetrics::default())),
            validator: None,
        }
    }

    /// Creates a new InMemoryBackend with an optional command validator
    pub fn with_validator(validator: Option<Arc<CommandValidator>>) -> Self {
        Self {
            metrics: Arc::new(Mutex::new(HashMap::new())),
            metric_values: Arc::new(Mutex::new(HashMap::new())),
            commands: Arc::new(Mutex::new(Vec::new())),
            command_queue: Arc::new(Mutex::new(Vec::new())),
            command_id_counter: Arc::new(Mutex::new(0)),
            status: Arc::new(Mutex::new(ChirpstackStatus::default())),
            health_metrics: Arc::new(Mutex::new(GatewayHealthMetrics::default())),
            validator,
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

    fn get_metric_value(&self, device_id: &str, metric_name: &str) -> Result<Option<MetricValue>, OpcGwError> {
        let values = self.metric_values.lock().map_err(|e| OpcGwError::Storage(format!("Lock error: {}", e)))?;
        Ok(values
            .get(device_id)
            .and_then(|device_values| device_values.get(metric_name).cloned()))
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

    fn update_command_status(&self, command_id: u64, status: CommandStatus, error_message: Option<String>) -> Result<(), OpcGwError> {
        let mut commands = self.commands.lock().map_err(|e| OpcGwError::Storage(format!("Lock error: {}", e)))?;
        if let Some(cmd) = commands.iter_mut().find(|c| c.id == command_id) {
            cmd.status = status;
            cmd.error_message = error_message;
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
        drop(metrics);

        // Note: InMemoryBackend stores MetricType enum, actual values stored via batch_write_metrics
        Ok(())
    }

    fn append_metric_history(&self, _device_id: &str, _metric_name: &str, _value: &MetricType, _timestamp: std::time::SystemTime) -> Result<(), OpcGwError> {
        // InMemoryBackend: no-op for testing (historical data not tracked in memory)
        Ok(())
    }

    fn batch_write_metrics(&self, metrics: Vec<crate::storage::BatchMetricWrite>) -> Result<(), OpcGwError> {
        // InMemoryBackend: batch insert all metrics (no transaction tracking)
        let mut type_map = self.metrics.lock().map_err(|e| OpcGwError::Storage(format!("Lock error: {}", e)))?;
        let mut value_map = self.metric_values.lock().map_err(|e| OpcGwError::Storage(format!("Lock error: {}", e)))?;

        for metric in metrics {
            // Store type
            type_map
                .entry(metric.device_id.clone())
                .or_insert_with(HashMap::new)
                .insert(metric.metric_name.clone(), metric.data_type);

            // Store value with metadata
            let metric_value = MetricValue {
                device_id: metric.device_id.clone(),
                metric_name: metric.metric_name.clone(),
                value: metric.value,
                timestamp: chrono::DateTime::<chrono::Utc>::from(metric.timestamp),
                data_type: metric.data_type,
            };
            value_map
                .entry(metric.device_id.clone())
                .or_insert_with(HashMap::new)
                .insert(metric.metric_name.clone(), metric_value);
        }
        Ok(())
    }

    fn load_all_metrics(&self) -> Result<Vec<MetricValue>, OpcGwError> {
        // InMemoryBackend: reconstruct metrics from internal storage
        let metrics = self.metrics.lock().map_err(|e| OpcGwError::Storage(format!("Lock error: {}", e)))?;
        let mut result = Vec::new();

        for (device_id, device_metrics) in metrics.iter() {
            for (metric_name, metric_type) in device_metrics.iter() {
                result.push(MetricValue {
                    device_id: device_id.clone(),
                    metric_name: metric_name.clone(),
                    value: metric_type.to_string(),
                    timestamp: Utc::now(),
                    data_type: *metric_type,
                });
            }
        }

        Ok(result)
    }

    fn prune_metric_history(&self) -> Result<u32, OpcGwError> {
        // InMemoryBackend: no-op (in-memory storage doesn't persist historical data)
        Ok(0)
    }

    fn enqueue_command(&self, mut command: Command) -> Result<u64, OpcGwError> {
        // Validate command parameters if validator is configured (Story 3-2)
        if let Some(validator) = &self.validator {
            validator.validate_command_parameters(
                &command.device_id,
                &command.command_name,
                &command.parameters,
            )?;
        } else {
            tracing::warn!("Command validator not configured; skipping parameter validation");
        }

        // Check for duplicate command (deduplication on pending commands)
        let queue = self.command_queue.lock()
            .map_err(|e| OpcGwError::Storage(format!("Lock error: {}", e)))?;

        if queue.iter().any(|cmd| cmd.command_hash == command.command_hash && cmd.status == CommandStatus::Pending) {
            return Err(OpcGwError::Storage(
                format!("Duplicate command already queued: {} for device {}",
                        command.command_name, command.device_id)
            ));
        }
        drop(queue); // Release the lock before acquiring it again for insertion

        let command_id = {
            let mut counter = self.command_id_counter.lock()
                .map_err(|e| OpcGwError::Storage(format!("Lock error: {}", e)))?;
            *counter += 1;
            *counter
        };
        command.id = command_id;

        let mut queue = self.command_queue.lock()
            .map_err(|e| OpcGwError::Storage(format!("Lock error: {}", e)))?;
        queue.push(command);
        Ok(command_id)
    }

    fn dequeue_command(&self) -> Result<Option<Command>, OpcGwError> {
        let mut queue = self.command_queue.lock()
            .map_err(|e| OpcGwError::Storage(format!("Lock error: {}", e)))?;

        // Find first pending command and mark as Sent (don't remove to preserve audit trail)
        if let Some(cmd) = queue.iter_mut().find(|cmd| cmd.status == CommandStatus::Pending) {
            let dequeued = cmd.clone();
            cmd.status = CommandStatus::Sent;
            Ok(Some(dequeued))
        } else {
            Ok(None)
        }
    }

    fn list_commands(&self, filter: &CommandFilter) -> Result<Vec<Command>, OpcGwError> {
        let queue = self.command_queue.lock()
            .map_err(|e| OpcGwError::Storage(format!("Lock error: {}", e)))?;

        let result: Vec<Command> = queue
            .iter()
            .filter(|cmd| {
                if let Some(device_id) = &filter.device_id {
                    if cmd.device_id != *device_id {
                        return false;
                    }
                }
                if let Some(status) = &filter.status {
                    if cmd.status != *status {
                        return false;
                    }
                }
                if let Some(command_name_contains) = &filter.command_name_contains {
                    if !cmd.command_name.contains(command_name_contains) {
                        return false;
                    }
                }
                if let Some(days) = filter.older_than_days {
                    let cutoff = Utc::now() - chrono::Duration::days(days as i64);
                    if cmd.enqueued_at >= cutoff {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();

        Ok(result)
    }

    fn get_queue_depth(&self) -> Result<usize, OpcGwError> {
        let queue = self.command_queue.lock()
            .map_err(|e| OpcGwError::Storage(format!("Lock error: {}", e)))?;

        Ok(queue.iter().filter(|cmd| cmd.status == CommandStatus::Pending).count())
    }

    fn mark_command_sent(&self, command_id: u64, chirpstack_result_id: &str) -> Result<(), OpcGwError> {
        let mut queue = self.command_queue.lock()
            .map_err(|e| OpcGwError::Storage(format!("Lock error: {}", e)))?;

        if let Some(cmd) = queue.iter_mut().find(|c| c.id == command_id) {
            cmd.status = CommandStatus::Sent;
            cmd.sent_at = Some(Utc::now());
            cmd.chirpstack_result_id = Some(chirpstack_result_id.to_string());
            Ok(())
        } else {
            Err(OpcGwError::Storage(format!("Command {} not found", command_id)))
        }
    }

    fn mark_command_confirmed(&self, command_id: u64) -> Result<(), OpcGwError> {
        let mut queue = self.command_queue.lock()
            .map_err(|e| OpcGwError::Storage(format!("Lock error: {}", e)))?;

        if let Some(cmd) = queue.iter_mut().find(|c| c.id == command_id) {
            cmd.status = CommandStatus::Confirmed;
            cmd.confirmed_at = Some(Utc::now());
            Ok(())
        } else {
            Err(OpcGwError::Storage(format!("Command {} not found", command_id)))
        }
    }

    fn mark_command_failed(&self, command_id: u64, error_message: &str) -> Result<(), OpcGwError> {
        let mut queue = self.command_queue.lock()
            .map_err(|e| OpcGwError::Storage(format!("Lock error: {}", e)))?;

        if let Some(cmd) = queue.iter_mut().find(|c| c.id == command_id) {
            cmd.status = CommandStatus::Failed;
            cmd.error_message = Some(error_message.to_string());
            Ok(())
        } else {
            Err(OpcGwError::Storage(format!("Command {} not found", command_id)))
        }
    }

    fn find_pending_confirmations(&self) -> Result<Vec<Command>, OpcGwError> {
        let queue = self.command_queue.lock()
            .map_err(|e| OpcGwError::Storage(format!("Lock error: {}", e)))?;

        let commands = queue.iter()
            .filter(|cmd| cmd.status == CommandStatus::Sent && cmd.confirmed_at.is_none())
            .cloned()
            .collect();

        Ok(commands)
    }

    fn find_timed_out_commands(&self, ttl_secs: u32) -> Result<Vec<Command>, OpcGwError> {
        let queue = self.command_queue.lock()
            .map_err(|e| OpcGwError::Storage(format!("Lock error: {}", e)))?;

        let now = Utc::now();
        let ttl = chrono::Duration::seconds(ttl_secs as i64);

        let commands = queue.iter()
            .filter(|cmd| {
                cmd.status == CommandStatus::Sent && cmd.sent_at
                    .map(|sent| now - sent > ttl)
                    .unwrap_or(false)
            })
            .cloned()
            .collect();

        Ok(commands)
    }

    fn update_gateway_status(
        &self,
        last_poll_timestamp: Option<DateTime<Utc>>,
        error_count: i32,
        chirpstack_available: bool,
    ) -> Result<(), OpcGwError> {
        let mut metrics = self.health_metrics.lock()
            .map_err(|_| OpcGwError::Storage("Failed to acquire lock on health_metrics".to_string()))?;

        // Only update timestamp if provided (None means poll failed, don't update)
        if last_poll_timestamp.is_some() {
            metrics.last_poll_timestamp = last_poll_timestamp;
        }
        metrics.error_count = error_count;
        metrics.chirpstack_available = chirpstack_available;

        Ok(())
    }

    fn get_gateway_health_metrics(&self) -> Result<(Option<DateTime<Utc>>, i32, bool), OpcGwError> {
        let metrics = self.health_metrics.lock()
            .map_err(|_| OpcGwError::Storage("Failed to acquire lock on health_metrics".to_string()))?;

        Ok((
            metrics.last_poll_timestamp,
            metrics.error_count,
            metrics.chirpstack_available,
        ))
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
        assert!(retrieved.server_available);
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
        backend.update_command_status(1, CommandStatus::Sent, None).unwrap();

        let pending = backend.get_pending_commands().unwrap();
        assert_eq!(pending.len(), 0); // No longer pending

        // Verify it's in sent status
        let all_commands = backend.commands.lock().unwrap();
        assert_eq!(all_commands[0].status, CommandStatus::Sent);
    }

    #[test]
    fn test_enqueue_command_assigns_id() {
        let backend = InMemoryBackend::new();
        let cmd = Command {
            id: 0,
            device_id: "device_123".to_string(),
            metric_id: "temperature".to_string(),
            command_name: "set_mode".to_string(),
            parameters: serde_json::json!({"mode": "auto"}),
            enqueued_at: chrono::Utc::now(),
            sent_at: None,
            confirmed_at: None,
            status: CommandStatus::Pending,
            error_message: None,
            command_hash: "hash123".to_string(),
            chirpstack_result_id: None,
        };

        let id = backend.enqueue_command(cmd).unwrap();
        assert_eq!(id, 1, "First command should get ID 1");
    }

    #[test]
    fn test_enqueue_command_increments_ids() {
        let backend = InMemoryBackend::new();

        for i in 1..=5 {
            let cmd = Command {
                id: 0,
                device_id: format!("device_{}", i),
                metric_id: "temperature".to_string(),
                command_name: "set_mode".to_string(),
                parameters: serde_json::json!({}),
                enqueued_at: chrono::Utc::now(),
                sent_at: None,
                confirmed_at: None,
                status: CommandStatus::Pending,
                error_message: None,
                command_hash: format!("hash_{}", i),
                chirpstack_result_id: None,
            };

            let id = backend.enqueue_command(cmd).unwrap();
            assert_eq!(id, i as u64, "Command {} should get ID {}", i, i);
        }
    }

    #[test]
    fn test_dequeue_command_fifo_order() {
        let backend = InMemoryBackend::new();

        for i in 1..=3 {
            let cmd = Command {
                id: 0,
                device_id: format!("device_{}", i),
                metric_id: "temperature".to_string(),
                command_name: "cmd".to_string(),
                parameters: serde_json::json!({}),
                enqueued_at: chrono::Utc::now(),
                sent_at: None,
                confirmed_at: None,
                status: CommandStatus::Pending,
                error_message: None,
                command_hash: format!("hash_{}", i),
                chirpstack_result_id: None,
            };
            backend.enqueue_command(cmd).unwrap();
        }

        // Dequeue should return in order
        let cmd1 = backend.dequeue_command().unwrap();
        assert!(cmd1.is_some());
        assert_eq!(cmd1.unwrap().id, 1);

        let cmd2 = backend.dequeue_command().unwrap();
        assert!(cmd2.is_some());
        assert_eq!(cmd2.unwrap().id, 2);

        let cmd3 = backend.dequeue_command().unwrap();
        assert!(cmd3.is_some());
        assert_eq!(cmd3.unwrap().id, 3);

        let cmd4 = backend.dequeue_command().unwrap();
        assert!(cmd4.is_none(), "No more pending commands");
    }

    #[test]
    fn test_dequeue_command_empty() {
        let backend = InMemoryBackend::new();
        let cmd = backend.dequeue_command().unwrap();
        assert!(cmd.is_none(), "Empty queue should return None");
    }

    #[test]
    fn test_dequeue_command_only_pending() {
        let backend = InMemoryBackend::new();

        let cmd1 = Command {
            id: 0,
            device_id: "device_1".to_string(),
            metric_id: "temperature".to_string(),
            command_name: "cmd".to_string(),
            parameters: serde_json::json!({}),
            enqueued_at: chrono::Utc::now(),
            sent_at: None,
            confirmed_at: None,
            status: CommandStatus::Pending,
            error_message: None,
            command_hash: "hash_1".to_string(),
            chirpstack_result_id: None,
        };

        let cmd2 = Command {
            id: 0,
            device_id: "device_2".to_string(),
            metric_id: "temperature".to_string(),
            command_name: "cmd".to_string(),
            parameters: serde_json::json!({}),
            enqueued_at: chrono::Utc::now(),
            sent_at: None,
            confirmed_at: None,
            status: CommandStatus::Sent,
            error_message: None,
            command_hash: "hash_2".to_string(),
            chirpstack_result_id: None,
        };

        backend.enqueue_command(cmd1).unwrap();
        backend.enqueue_command(cmd2).unwrap();

        let dequeued = backend.dequeue_command().unwrap();
        assert!(dequeued.is_some());
        assert_eq!(dequeued.unwrap().id, 1, "Should dequeue first (Pending) command");
    }

    #[test]
    fn test_list_commands_filter_by_device_id() {
        let backend = InMemoryBackend::new();

        for i in 1..=3 {
            let device_id = if i <= 2 { "device_a" } else { "device_b" };
            let cmd = Command {
                id: 0,
                device_id: device_id.to_string(),
                metric_id: "temperature".to_string(),
                command_name: "cmd".to_string(),
                parameters: serde_json::json!({}),
                enqueued_at: chrono::Utc::now(),
                sent_at: None,
                confirmed_at: None,
                status: CommandStatus::Pending,
                error_message: None,
                command_hash: format!("hash_{}", i),
                chirpstack_result_id: None,
            };
            backend.enqueue_command(cmd).unwrap();
        }

        let filter = CommandFilter {
            device_id: Some("device_a".to_string()),
            status: None,
            command_name_contains: None,
            older_than_days: None,
        };

        let commands = backend.list_commands(&filter).unwrap();
        assert_eq!(commands.len(), 2);
        assert!(commands.iter().all(|c| c.device_id == "device_a"));
    }

    #[test]
    fn test_list_commands_filter_by_status() {
        let backend = InMemoryBackend::new();

        for i in 1..=3 {
            let status = if i == 1 { CommandStatus::Sent } else { CommandStatus::Pending };
            let cmd = Command {
                id: 0,
                device_id: format!("device_{}", i),
                metric_id: "temperature".to_string(),
                command_name: "cmd".to_string(),
                parameters: serde_json::json!({}),
                enqueued_at: chrono::Utc::now(),
                sent_at: None,
                confirmed_at: None,
                status,
                error_message: None,
                command_hash: format!("hash_{}", i),
                chirpstack_result_id: None,
            };
            backend.enqueue_command(cmd).unwrap();
        }

        let filter = CommandFilter {
            device_id: None,
            status: Some(CommandStatus::Pending),
            command_name_contains: None,
            older_than_days: None,
        };

        let commands = backend.list_commands(&filter).unwrap();
        assert_eq!(commands.len(), 2);
        assert!(commands.iter().all(|c| c.status == CommandStatus::Pending));
    }

    #[test]
    fn test_list_commands_filter_by_command_name_contains() {
        let backend = InMemoryBackend::new();

        for (i, name) in ["set_temperature", "set_mode", "get_status"].iter().enumerate() {
            let cmd = Command {
                id: 0,
                device_id: "device_1".to_string(),
                metric_id: "temperature".to_string(),
                command_name: name.to_string(),
                parameters: serde_json::json!({}),
                enqueued_at: chrono::Utc::now(),
                sent_at: None,
                confirmed_at: None,
                status: CommandStatus::Pending,
                error_message: None,
                command_hash: format!("hash_{}", i),
                chirpstack_result_id: None,
            };
            backend.enqueue_command(cmd).unwrap();
        }

        let filter = CommandFilter {
            device_id: None,
            status: None,
            command_name_contains: Some("set_".to_string()),
            older_than_days: None,
        };

        let commands = backend.list_commands(&filter).unwrap();
        assert_eq!(commands.len(), 2);
        assert!(commands.iter().all(|c| c.command_name.contains("set_")));
    }

    #[test]
    fn test_list_commands_multiple_filters() {
        let backend = InMemoryBackend::new();

        let cmd1 = Command {
            id: 0,
            device_id: "device_a".to_string(),
            metric_id: "temperature".to_string(),
            command_name: "set_mode".to_string(),
            parameters: serde_json::json!({}),
            enqueued_at: chrono::Utc::now(),
            sent_at: None,
            confirmed_at: None,
            status: CommandStatus::Pending,
            error_message: None,
            command_hash: "hash_1".to_string(),
            chirpstack_result_id: None,
        };

        let cmd2 = Command {
            id: 0,
            device_id: "device_a".to_string(),
            metric_id: "humidity".to_string(),
            command_name: "set_mode".to_string(),
            parameters: serde_json::json!({}),
            enqueued_at: chrono::Utc::now(),
            sent_at: None,
            confirmed_at: None,
            status: CommandStatus::Sent,
            error_message: None,
            command_hash: "hash_2".to_string(),
            chirpstack_result_id: None,
        };

        let cmd3 = Command {
            id: 0,
            device_id: "device_b".to_string(),
            metric_id: "temperature".to_string(),
            command_name: "set_mode".to_string(),
            parameters: serde_json::json!({}),
            enqueued_at: chrono::Utc::now(),
            sent_at: None,
            confirmed_at: None,
            status: CommandStatus::Pending,
            error_message: None,
            command_hash: "hash_3".to_string(),
            chirpstack_result_id: None,
        };

        backend.enqueue_command(cmd1).unwrap();
        backend.enqueue_command(cmd2).unwrap();
        backend.enqueue_command(cmd3).unwrap();

        let filter = CommandFilter {
            device_id: Some("device_a".to_string()),
            status: Some(CommandStatus::Pending),
            command_name_contains: None,
            older_than_days: None,
        };

        let commands = backend.list_commands(&filter).unwrap();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].device_id, "device_a");
        assert_eq!(commands[0].status, CommandStatus::Pending);
    }

    #[test]
    fn test_get_queue_depth_empty() {
        let backend = InMemoryBackend::new();
        let depth = backend.get_queue_depth().unwrap();
        assert_eq!(depth, 0);
    }

    #[test]
    fn test_get_queue_depth_pending_only() {
        let backend = InMemoryBackend::new();

        for i in 1..=5 {
            let status = if i > 3 { CommandStatus::Sent } else { CommandStatus::Pending };
            let cmd = Command {
                id: 0,
                device_id: format!("device_{}", i),
                metric_id: "temperature".to_string(),
                command_name: "cmd".to_string(),
                parameters: serde_json::json!({}),
                enqueued_at: chrono::Utc::now(),
                sent_at: None,
                confirmed_at: None,
                status,
                error_message: None,
                command_hash: format!("hash_{}", i),
                chirpstack_result_id: None,
            };
            backend.enqueue_command(cmd).unwrap();
        }

        let depth = backend.get_queue_depth().unwrap();
        assert_eq!(depth, 3, "Should count only pending commands");
    }

    #[test]
    fn test_get_queue_depth_after_dequeue() {
        let backend = InMemoryBackend::new();

        for i in 1..=3 {
            let cmd = Command {
                id: 0,
                device_id: format!("device_{}", i),
                metric_id: "temperature".to_string(),
                command_name: "cmd".to_string(),
                parameters: serde_json::json!({}),
                enqueued_at: chrono::Utc::now(),
                sent_at: None,
                confirmed_at: None,
                status: CommandStatus::Pending,
                error_message: None,
                command_hash: format!("hash_{}", i),
                chirpstack_result_id: None,
            };
            backend.enqueue_command(cmd).unwrap();
        }

        assert_eq!(backend.get_queue_depth().unwrap(), 3);

        backend.dequeue_command().unwrap();
        assert_eq!(backend.get_queue_depth().unwrap(), 2);

        backend.dequeue_command().unwrap();
        assert_eq!(backend.get_queue_depth().unwrap(), 1);
    }

    #[test]
    fn test_concurrent_enqueue() {
        use std::sync::Arc;
        use std::thread;

        let backend = Arc::new(InMemoryBackend::new());
        let mut handles = vec![];

        // Spawn 5 threads, each enqueuing 10 commands
        for t in 0..5 {
            let backend_clone = Arc::clone(&backend);
            let handle = thread::spawn(move || {
                for i in 0..10 {
                    let cmd = Command {
                        id: 0,
                        device_id: format!("device_{}_{}", t, i),
                        metric_id: "temperature".to_string(),
                        command_name: "cmd".to_string(),
                        parameters: serde_json::json!({}),
                        enqueued_at: chrono::Utc::now(),
                        sent_at: None,
                        confirmed_at: None,
                        status: CommandStatus::Pending,
                        error_message: None,
                        command_hash: format!("hash_{}_{}", t, i),
                        chirpstack_result_id: None,
                    };
                    backend_clone.enqueue_command(cmd).expect("Should enqueue");
                }
            });
            handles.push(handle);
        }

        // Wait for all threads to complete
        for handle in handles {
            handle.join().expect("Thread should complete");
        }

        // Verify all 50 commands are in the queue
        assert_eq!(backend.get_queue_depth().unwrap(), 50);
    }
}

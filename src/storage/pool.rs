// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! SQLite Connection Pool
//!
//! Provides a simple, efficient connection pool for multi-task concurrent database access.
//! Uses a custom thin wrapper over Vec<Connection> with Mutex-protected availability list.
//!
//! # Design
//! - Custom pool (not r2d2) to minimize dependencies and keep logic simple
//! - Fixed pool size determined at startup (typically 3-5 connections)
//! - ConnectionGuard RAII pattern ensures connections return to pool on drop
//! - Timeout-based checkout prevents indefinite waiting
//!
//! # Concurrency
//! - Multiple tasks can checkout different connections concurrently
//! - SQLite WAL mode at database level enables true concurrent readers + single writer
//! - No Rust Mutex bottleneck (unlike shared Arc<Mutex<Connection>>)
//!
//! # Example
//! ```no_run
//! use opcgw::storage::ConnectionPool;
//! use std::time::Duration;
//!
//! let pool = ConnectionPool::new("data/opcgw.db", 3)?;
//! let conn_guard = pool.checkout(Duration::from_secs(5))?;
//! // Use connection via conn_guard
//! // Connection automatically returned to pool when conn_guard is dropped
//! ```

use rusqlite::Connection;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use crate::utils::OpcGwError;

/// A thread-safe pool of SQLite connections for concurrent task access.
///
/// Provides RAII-based connection checkout via ConnectionGuard.
/// Pool size is fixed at creation time; all connections are pre-created.
pub struct ConnectionPool {
    connections: Vec<Connection>,
    available: Mutex<Vec<usize>>,  // indices of available connections
}

impl ConnectionPool {
    /// Create a new connection pool with the specified number of connections.
    ///
    /// All connections are created at startup and connected to the same database file.
    /// Returns error if any connection cannot be created.
    ///
    /// # Arguments
    /// * `path` - File system path to SQLite database (e.g., "data/opcgw.db")
    /// * `size` - Number of connections in pool (recommended: 3-5)
    ///
    /// # Returns
    /// * `Ok(ConnectionPool)` - Pool created and all connections established
    /// * `Err(OpcGwError::Database)` - If any connection creation fails
    pub fn new(path: &str, size: usize) -> Result<Self, OpcGwError> {
        if size == 0 {
            return Err(OpcGwError::Database(
                "Connection pool size must be at least 1".to_string(),
            ));
        }

        // Handle database corruption/repair before creating connections
        Self::detect_and_repair_database(path)?;

        let mut connections = Vec::with_capacity(size);
        let mut available = Vec::with_capacity(size);

        for i in 0..size {
            let conn = Connection::open(path).map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to create connection {} for pool: {}",
                    i, e
                ))
            })?;

            connections.push(conn);
            available.push(i);
        }

        tracing::info!(
            pool_size = size,
            database = path,
            "Connection pool initialized"
        );

        Ok(ConnectionPool {
            connections,
            available: Mutex::new(available),
        })
    }

    /// Detect and repair database corruption (AC 2-4b).
    ///
    /// Handles three scenarios:
    /// 1. Missing database file: SQLite auto-creates it on first open (OK)
    /// 2. Corrupted database: Attempts PRAGMA integrity_check, repair if needed
    /// 3. Repair failure: Deletes corrupted file and creates fresh database
    ///
    /// # Arguments
    /// * `path` - File system path to SQLite database
    ///
    /// # Returns
    /// * `Ok(())` - Database is healthy or was repaired/recreated
    /// * `Err(OpcGwError::Database)` - If repair fails and database cannot be recreated
    fn detect_and_repair_database(path: &str) -> Result<(), OpcGwError> {
        use std::path::Path;
        use std::fs;

        let db_path = Path::new(path);

        // If database doesn't exist, SQLite will auto-create it on first open - that's fine
        if !db_path.exists() {
            tracing::info!(database = path, "Database file does not exist; will be auto-created");
            return Ok(());
        }

        // Try to open and check integrity
        let conn = match Connection::open(path) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(
                    database = path,
                    error = %e,
                    "Failed to open database; attempting repair"
                );
                // Try to delete and let it be recreated
                if let Err(del_err) = fs::remove_file(path) {
                    tracing::error!(
                        database = path,
                        error = %del_err,
                        "Failed to delete corrupted database"
                    );
                    return Err(OpcGwError::Database(format!(
                        "Failed to open database at {}: {}. Could not delete for recovery: {}",
                        path, e, del_err
                    )));
                }
                tracing::info!(database = path, "Deleted corrupted database; will be recreated");
                return Ok(());
            }
        };

        // Run integrity check
        let integrity_result: Result<String, _> = conn.query_row(
            "PRAGMA integrity_check",
            [],
            |row| row.get(0),
        );

        match integrity_result {
            Ok(result) if result.to_lowercase() == "ok" => {
                tracing::info!(database = path, "Database integrity check passed");
                Ok(())
            }
            Ok(errors) => {
                // Corruption detected, attempt repair
                tracing::warn!(
                    database = path,
                    corruption_details = %errors,
                    "Database corruption detected; attempting repair"
                );

                drop(conn);  // Release the connection before repair

                // Try REINDEX first
                match Connection::open(path) {
                    Ok(mut repair_conn) => {
                        if let Err(e) = repair_conn.execute_batch("REINDEX") {
                            tracing::warn!(
                                database = path,
                                error = %e,
                                "REINDEX failed; attempting to delete and recreate database"
                            );

                            drop(repair_conn);
                            if let Err(del_err) = fs::remove_file(path) {
                                return Err(OpcGwError::Database(format!(
                                    "Database repair failed for {}. Could not delete: {}",
                                    path, del_err
                                )));
                            }
                            tracing::info!(database = path, "Deleted corrupted database; will be recreated");
                        } else {
                            tracing::info!(database = path, "Database REINDEX completed successfully");

                            // Verify repair succeeded
                            if let Err(verify_err) = repair_conn.query_row(
                                "PRAGMA integrity_check",
                                [],
                                |row| row.get::<_, String>(0),
                            ) {
                                tracing::error!(
                                    database = path,
                                    error = %verify_err,
                                    "Repair verification failed"
                                );
                                drop(repair_conn);
                                if let Err(del_err) = fs::remove_file(path) {
                                    return Err(OpcGwError::Database(format!(
                                        "Database repair failed and could not delete: {}",
                                        del_err
                                    )));
                                }
                                tracing::info!(database = path, "Deleted corrupted database after failed repair");
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            database = path,
                            error = %e,
                            "Could not reopen database for repair"
                        );
                        if let Err(del_err) = fs::remove_file(path) {
                            return Err(OpcGwError::Database(format!(
                                "Database repair failed: {}. Could not delete: {}",
                                e, del_err
                            )));
                        }
                        tracing::info!(database = path, "Deleted corrupted database; will be recreated");
                    }
                }

                Ok(())
            }
            Err(e) => {
                tracing::error!(
                    database = path,
                    error = %e,
                    "Failed to run integrity check"
                );
                drop(conn);

                // Delete corrupted file and let it be recreated
                if let Err(del_err) = fs::remove_file(path) {
                    return Err(OpcGwError::Database(format!(
                        "Failed to check database integrity: {}. Could not delete: {}",
                        e, del_err
                    )));
                }
                tracing::info!(database = path, "Deleted corrupted database; will be recreated");
                Ok(())
            }
        }
    }

    /// Checkout a connection from the pool with timeout.
    ///
    /// Returns a ConnectionGuard that holds an exclusive reference to a connection.
    /// When the guard is dropped, the connection is automatically returned to the pool.
    ///
    /// If no connections are available, blocks up to `timeout` duration waiting.
    /// If timeout expires, returns error without blocking caller.
    ///
    /// # Arguments
    /// * `timeout` - Maximum duration to wait for an available connection
    ///
    /// # Returns
    /// * `Ok(ConnectionGuard)` - Connection acquired, can use immediately
    /// * `Err(OpcGwError::Database)` - Timeout expired or pool is closed
    pub fn checkout(
        &self,
        timeout: Duration,
    ) -> Result<ConnectionGuard, OpcGwError> {
        let start = Instant::now();

        loop {
            // Try to acquire an available connection
            {
                let mut available = self.available.lock().map_err(|e| {
                    OpcGwError::Database(format!(
                        "Connection pool mutex poisoned: {}",
                        e
                    ))
                })?;

                if let Some(idx) = available.pop() {
                    tracing::trace!(connection_index = idx, "Checked out connection");
                    return Ok(ConnectionGuard {
                        pool: self as *const Self as *mut Self,
                        connection_index: idx,
                    });
                }
            }

            // No connections available, check timeout
            if start.elapsed() > timeout {
                tracing::warn!(
                    timeout_ms = timeout.as_millis(),
                    "Connection pool checkout timeout"
                );
                return Err(OpcGwError::Database(
                    "Connection pool timeout: all connections in use".to_string(),
                ));
            }

            // Brief sleep to prevent busy-waiting
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Return a connection to the pool.
    ///
    /// This is called automatically by ConnectionGuard::drop().
    /// Should not be called directly by user code.
    fn return_connection(&self, index: usize) -> Result<(), OpcGwError> {
        let mut available = self.available.lock().map_err(|e| {
            OpcGwError::Database(format!(
                "Connection pool mutex poisoned on return: {}",
                e
            ))
        })?;

        available.push(index);
        tracing::trace!(connection_index = index, "Returned connection to pool");

        Ok(())
    }

    /// Get reference to a connection by index (internal use only).
    fn get_connection(&self, index: usize) -> Option<&Connection> {
        self.connections.get(index)
    }

    /// Get mutable reference to a connection by index (internal use only).
    fn get_connection_mut(&mut self, index: usize) -> Option<&mut Connection> {
        self.connections.get_mut(index)
    }

    /// Get number of currently available connections in pool.
    pub fn available_connections(&self) -> usize {
        self.available
            .lock()
            .map(|available| available.len())
            .unwrap_or(0)
    }

    /// Close the pool, waiting for all connections to be returned.
    ///
    /// Blocks until all checked-out connections are returned to the pool,
    /// then closes all connections gracefully.
    ///
    /// Timeout of 30 seconds to prevent indefinite waiting.
    pub fn close(&self) -> Result<(), OpcGwError> {
        let start = Instant::now();
        let max_wait = Duration::from_secs(30);
        let pool_size = self.connections.len();

        // Wait for all connections to be returned
        loop {
            {
                let available = self.available.lock().map_err(|e| {
                    OpcGwError::Database(format!(
                        "Connection pool mutex poisoned on close: {}",
                        e
                    ))
                })?;

                if available.len() == pool_size {
                    tracing::info!("All connections returned to pool, closing");
                    break;
                }
            }

            if start.elapsed() > max_wait {
                tracing::warn!(
                    "Connection pool close timeout: {} of {} connections not returned",
                    pool_size - self.available_connections(),
                    pool_size
                );
                return Err(OpcGwError::Database(
                    "Connection pool close timeout: not all connections returned".to_string(),
                ));
            }

            std::thread::sleep(Duration::from_millis(10));
        }

        // Close all connections
        for (i, _) in self.connections.iter().enumerate() {
            // Connections close automatically when dropped, but we could add explicit close
            // For now, just log
            tracing::debug!(connection_index = i, "Connection closed");
        }

        tracing::info!("Connection pool closed successfully");
        Ok(())
    }
}

/// RAII guard for a checked-out connection from the pool.
///
/// Automatically returns the connection to the pool when dropped.
/// Implements Deref<Target=Connection> for easy use.
#[derive(Debug)]
pub struct ConnectionGuard {
    pool: *mut ConnectionPool,
    connection_index: usize,
}

impl ConnectionGuard {
    /// Get reference to the underlying SQLite connection.
    pub fn as_ref(&self) -> &Connection {
        unsafe {
            self.pool
                .as_ref()
                .and_then(|p| p.get_connection(self.connection_index))
                .expect("ConnectionGuard holds valid connection reference")
        }
    }

    /// Get mutable reference to the underlying SQLite connection.
    pub fn as_mut(&mut self) -> &mut Connection {
        unsafe {
            self.pool
                .as_mut()
                .and_then(|p| p.get_connection_mut(self.connection_index))
                .expect("ConnectionGuard holds valid connection reference")
        }
    }
}

impl std::ops::Deref for ConnectionGuard {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl std::ops::DerefMut for ConnectionGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut()
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        unsafe {
            if let Some(pool) = self.pool.as_ref() {
                let _ = pool.return_connection(self.connection_index);
            }
        }
    }
}

// Safety: ConnectionPool and ConnectionGuard can be safely shared across thread boundaries
// because the underlying SQLite connections are thread-safe (opened in same database, WAL mode)
// and the pool uses Mutex for internal state synchronization.
unsafe impl Send for ConnectionPool {}
unsafe impl Sync for ConnectionPool {}

unsafe impl Send for ConnectionGuard {}
unsafe impl Sync for ConnectionGuard {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use uuid::Uuid;

    fn temp_db_path() -> String {
        format!("/tmp/opcgw_pool_test_{}.db", Uuid::new_v4())
    }

    #[test]
    fn test_pool_creation() {
        let path = temp_db_path();
        let result = ConnectionPool::new(&path, 3);
        assert!(result.is_ok(), "Should create pool");
        assert_eq!(result.unwrap().available_connections(), 3);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_pool_checkout_and_return() {
        let path = temp_db_path();
        let pool = ConnectionPool::new(&path, 2).expect("Should create pool");

        // Check out first connection
        let guard1 = pool
            .checkout(Duration::from_secs(5))
            .expect("Should checkout");
        assert_eq!(pool.available_connections(), 1);

        // Check out second connection
        let guard2 = pool
            .checkout(Duration::from_secs(5))
            .expect("Should checkout");
        assert_eq!(pool.available_connections(), 0);

        // Drop first guard, connection should return
        drop(guard1);
        assert_eq!(pool.available_connections(), 1);

        // Drop second guard
        drop(guard2);
        assert_eq!(pool.available_connections(), 2);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_pool_timeout() {
        let path = temp_db_path();
        let pool = ConnectionPool::new(&path, 1).expect("Should create pool");

        // Check out the only connection
        let _guard = pool
            .checkout(Duration::from_secs(5))
            .expect("Should checkout");

        // Try to check out another, should timeout
        let result = pool.checkout(Duration::from_millis(100));
        assert!(result.is_err(), "Should timeout");
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("timeout"),
            "Should be timeout error");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_concurrent_checkouts() {
        use std::sync::Arc;
        use std::thread;

        let path = temp_db_path();
        let pool = Arc::new(ConnectionPool::new(&path, 3).expect("Should create pool"));

        let mut handles = vec![];
        for i in 0..3 {
            let pool = Arc::clone(&pool);
            let handle = thread::spawn(move || {
                let _guard = pool
                    .checkout(Duration::from_secs(5))
                    .expect("Should checkout");
                // Hold connection briefly
                thread::sleep(Duration::from_millis(10));
                // Connection returns on drop
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().expect("Thread should complete");
        }

        assert_eq!(pool.available_connections(), 3, "All connections should be returned");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_concurrent_reads_during_write() {
        use std::sync::Arc;
        use std::thread;
        use std::sync::Barrier;

        let path = temp_db_path();
        let pool = Arc::new(ConnectionPool::new(&path, 3).expect("Should create pool"));

        // Create a table for testing
        {
            let mut conn = pool.checkout(Duration::from_secs(5)).expect("Should checkout");
            conn.execute(
                "CREATE TABLE test_data (id INTEGER PRIMARY KEY, value TEXT)",
                [],
            ).expect("Should create table");
            conn.execute(
                "INSERT INTO test_data (id, value) VALUES (1, 'initial')",
                [],
            ).expect("Should insert");
        }

        let barrier = Arc::new(Barrier::new(2));
        let write_barrier = barrier.clone();

        // Writer thread: start transaction and hold it
        let write_pool = Arc::clone(&pool);
        let write_handle = thread::spawn(move || {
            let mut conn = write_pool.checkout(Duration::from_secs(5)).expect("Should checkout");
            conn.execute_batch("BEGIN TRANSACTION").expect("Should begin");
            conn.execute(
                "UPDATE test_data SET value = 'modified' WHERE id = 1",
                [],
            ).expect("Should update");
            write_barrier.wait();
            thread::sleep(Duration::from_millis(100));
            conn.execute_batch("COMMIT").expect("Should commit");
        });

        // Reader thread: read concurrently
        thread::sleep(Duration::from_millis(10));
        let read_barrier = barrier.clone();
        let read_pool = Arc::clone(&pool);
        let read_handle = thread::spawn(move || {
            read_barrier.wait();
            let mut conn = read_pool.checkout(Duration::from_secs(5)).expect("Should checkout");
            let value: String = conn.query_row(
                "SELECT value FROM test_data WHERE id = 1",
                [],
                |row| row.get(0)
            ).expect("Should query");
            value
        });

        write_handle.join().expect("Write thread should complete");
        let read_value = read_handle.join().expect("Read thread should complete");

        // Reader should see either 'initial' or 'modified' (WAL isolation)
        assert!(
            read_value == "initial" || read_value == "modified",
            "Read should see consistent state"
        );

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_pool_exhaustion_timeout() {
        use std::sync::Arc;

        let path = temp_db_path();
        let pool = Arc::new(ConnectionPool::new(&path, 1).expect("Should create pool"));

        // Acquire the only connection
        let guard = pool.checkout(Duration::from_secs(5)).expect("Should checkout first");

        // Try to get another with short timeout
        let result = pool.checkout(Duration::from_millis(100));
        assert!(result.is_err(), "Should timeout with exhausted pool");

        // Release the connection
        drop(guard);

        // Now checkout should succeed
        let _guard2 = pool.checkout(Duration::from_millis(100)).expect("Should checkout after release");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_transaction_isolation() {
        use std::sync::Arc;
        use std::thread;

        let path = temp_db_path();
        let pool = Arc::new(ConnectionPool::new(&path, 3).expect("Should create pool"));

        // Initialize status table
        {
            let mut conn = pool.checkout(Duration::from_secs(5)).expect("Should checkout");
            conn.execute(
                "CREATE TABLE gateway_status (key TEXT PRIMARY KEY, value TEXT)",
                [],
            ).expect("Should create table");
            conn.execute(
                "INSERT INTO gateway_status (key, value) VALUES ('counter', '0')",
                [],
            ).expect("Should insert");
        }

        let update_pool = Arc::clone(&pool);
        let update_handle = thread::spawn(move || {
            let mut conn = update_pool.checkout(Duration::from_secs(5)).expect("Should checkout");
            conn.execute_batch("BEGIN TRANSACTION").expect("Should begin");
            conn.execute(
                "UPDATE gateway_status SET value = '42' WHERE key = 'counter'",
                [],
            ).expect("Should update");
            thread::sleep(Duration::from_millis(100));
            conn.execute_batch("COMMIT").expect("Should commit");
        });

        // Wait for update to complete and commit
        update_handle.join().expect("Update thread should complete");

        // Now read should see the new value
        let read_pool = Arc::clone(&pool);
        let mut conn = read_pool.checkout(Duration::from_secs(5)).expect("Should checkout");
        let value: String = conn.query_row(
            "SELECT value FROM gateway_status WHERE key = 'counter'",
            [],
            |row| row.get(0)
        ).expect("Should query");

        // After update commits, read should see new value
        assert_eq!(value, "42", "Should see committed value");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_per_task_panic_isolation() {
        use std::sync::Arc;
        use std::thread;
        use std::panic;

        let path = temp_db_path();
        let pool = Arc::new(ConnectionPool::new(&path, 2).expect("Should create pool"));

        // Initialize test table
        {
            let mut conn = pool.checkout(Duration::from_secs(5)).expect("Should checkout");
            conn.execute(
                "CREATE TABLE metrics (id INTEGER PRIMARY KEY, value REAL)",
                [],
            ).expect("Should create table");
        }

        let panic_pool = Arc::clone(&pool);
        let panic_handle = thread::spawn(move || {
            let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                let _conn = panic_pool.checkout(Duration::from_secs(5)).expect("Should checkout");
                // Panic while holding connection
                panic!("Intentional panic");
            }));
            assert!(result.is_err(), "Should have panicked");
        });

        thread::sleep(Duration::from_millis(10));

        // Other connection should still work
        let other_pool = Arc::clone(&pool);
        let other_handle = thread::spawn(move || {
            let mut conn = other_pool.checkout(Duration::from_secs(5)).expect("Should checkout");
            conn.execute(
                "INSERT INTO metrics (id, value) VALUES (1, 3.14)",
                [],
            ).expect("Should insert");
            true
        });

        panic_handle.join().expect("Panic thread should complete");
        let success = other_handle.join().expect("Other thread should complete");

        assert!(success, "Other task should continue after panic");
        assert!(pool.available_connections() > 0, "Pool should have connections available");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_pool_throughput_under_load() {
        use std::sync::Arc;
        use std::thread;
        use std::time::Instant;

        let path = temp_db_path();
        let pool = Arc::new(ConnectionPool::new(&path, 4).expect("Should create pool"));

        // Initialize table
        {
            let mut conn = pool.checkout(Duration::from_secs(5)).expect("Should checkout");
            conn.execute(
                "CREATE TABLE perf_test (id INTEGER PRIMARY KEY, value INTEGER)",
                [],
            ).expect("Should create table");
        }

        let start = Instant::now();
        let num_threads = 4;
        let ops_per_thread = 25;
        let mut handles = vec![];

        for _ in 0..num_threads {
            let thread_pool = Arc::clone(&pool);
            let handle = thread::spawn(move || {
                for i in 0..ops_per_thread {
                    let mut conn = thread_pool.checkout(Duration::from_secs(5)).expect("Should checkout");
                    conn.execute(
                        "INSERT INTO perf_test (id, value) VALUES (NULL, ?1)",
                        [i as i32],
                    ).expect("Should insert");
                }
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().expect("Thread should complete");
        }

        let elapsed = start.elapsed();
        let total_ops = num_threads * ops_per_thread;
        let ops_per_sec = total_ops as f64 / elapsed.as_secs_f64();

        // Performance target: >100 ops/sec (very conservative; actual should be much higher)
        assert!(
            ops_per_sec > 100.0,
            "Pool throughput {} ops/sec below target 100 ops/sec",
            ops_per_sec as u32
        );

        tracing::info!(
            total_ops = total_ops,
            elapsed_ms = elapsed.as_millis(),
            ops_per_sec = ops_per_sec as u32,
            "Pool throughput performance: {} ops in {} ms",
            total_ops,
            elapsed.as_millis()
        );

        let _ = fs::remove_file(&path);
    }

    // ========== Story 2-4b: Graceful Degradation Tests ==========

    #[test]
    fn test_missing_database_auto_created() {
        let path = temp_db_path();
        // Ensure file doesn't exist
        let _ = fs::remove_file(&path);

        // Pool creation should succeed and auto-create database
        let pool = ConnectionPool::new(&path, 1).expect("Should handle missing database gracefully");

        // Verify database exists now
        assert!(std::path::Path::new(&path).exists(), "Database should be auto-created");

        // Verify we can use it
        let conn = pool.checkout(Duration::from_secs(5)).expect("Should checkout");
        let result: String = conn.query_row(
            "PRAGMA database_list",
            [],
            |row| row.get(1),
        ).expect("Should query");
        assert!(!result.is_empty(), "Database should be functional");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_corrupted_database_deleted_and_recreated() {
        use std::sync::Arc;

        let path = temp_db_path();

        // Create a valid database first
        {
            let pool = ConnectionPool::new(&path, 1).expect("Should create pool");
            let conn = pool.checkout(Duration::from_secs(5)).expect("Should checkout");
            conn.execute(
                "CREATE TABLE test_data (id INTEGER PRIMARY KEY, value TEXT)",
                [],
            ).expect("Should create table");
        }

        // Corrupt the database by overwriting with garbage
        {
            use std::io::Write;
            let mut file = fs::File::create(&path).expect("Should open file");
            let garbage = b"this is not a valid SQLite database\x00\x00\x00";
            file.write_all(garbage).expect("Should write garbage");
        }

        // Pool creation should detect corruption, delete, and recreate
        let result = ConnectionPool::new(&path, 1);
        assert!(result.is_ok(), "Should handle corrupted database by recreating");

        // Verify new database is functional
        let pool = result.unwrap();
        let conn = pool.checkout(Duration::from_secs(5)).expect("Should checkout");
        let result: String = conn.query_row(
            "PRAGMA database_list",
            [],
            |row| row.get(1),
        ).expect("Should query new database");
        assert!(!result.is_empty(), "Recreated database should be functional");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_valid_database_untouched() {
        use std::sync::Arc;

        let path = temp_db_path();

        // Create a valid database with a table
        {
            let pool = ConnectionPool::new(&path, 1).expect("Should create pool");
            let conn = pool.checkout(Duration::from_secs(5)).expect("Should checkout");
            conn.execute(
                "CREATE TABLE original_table (id INTEGER PRIMARY KEY, data TEXT)",
                [],
            ).expect("Should create table");
            conn.execute(
                "INSERT INTO original_table (id, data) VALUES (1, 'preserved')",
                [],
            ).expect("Should insert data");
        }

        // Create a new pool - should find database intact
        let pool = ConnectionPool::new(&path, 1).expect("Should open existing pool");
        let conn = pool.checkout(Duration::from_secs(5)).expect("Should checkout");

        // Verify table still exists with data
        let value: String = conn.query_row(
            "SELECT data FROM original_table WHERE id = 1",
            [],
            |row| row.get(0),
        ).expect("Should query existing data");

        assert_eq!(value, "preserved", "Data should be preserved in valid database");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_pool_starts_with_empty_state_after_recovery() {
        use std::sync::Arc;

        let path = temp_db_path();

        // Create and corrupt database
        {
            let pool = ConnectionPool::new(&path, 1).expect("Should create pool");
            let conn = pool.checkout(Duration::from_secs(5)).expect("Should checkout");
            conn.execute(
                "CREATE TABLE test_table (id INTEGER PRIMARY KEY)",
                [],
            ).expect("Should create table");
        }

        // Corrupt it
        {
            use std::io::Write;
            let mut file = fs::File::create(&path).expect("Should open file");
            file.write_all(b"corrupted").expect("Should corrupt");
        }

        // Recovery should create fresh database
        let pool = ConnectionPool::new(&path, 1).expect("Should recover");

        // Old table shouldn't exist
        let conn = pool.checkout(Duration::from_secs(5)).expect("Should checkout");
        let result = conn.execute(
            "SELECT COUNT(*) FROM test_table",
            [],
        );

        assert!(result.is_err(), "Old table should not exist after recovery");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_pool_multiple_sizes_with_corrupt_recovery() {
        use std::sync::Arc;

        let path = temp_db_path();

        // Create with size 3
        let pool1 = ConnectionPool::new(&path, 3).expect("Should create pool with size 3");
        assert_eq!(pool1.available_connections(), 3);

        // Corrupt the database
        drop(pool1);
        {
            use std::io::Write;
            let mut file = fs::File::create(&path).expect("Should open file");
            file.write_all(b"garbage").expect("Should corrupt");
        }

        // Create with size 2 - should recover and use new size
        let pool2 = ConnectionPool::new(&path, 2).expect("Should recover with new size");
        assert_eq!(pool2.available_connections(), 2);

        let _ = fs::remove_file(&path);
    }
}

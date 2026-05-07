// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! TOML round-trip helper for CRUD-driven configuration mutations
//! (Story 9-4).
//!
//! `figment` (`src/config.rs`) owns the **read** side — merging
//! `config.toml` with `OPCGW_*` env vars into an `AppConfig`. This
//! module owns the **write** side via `toml_edit`, which preserves
//! operator-edited comments, key order, and whitespace on round-trip
//! (figment cannot do this; plain `toml::to_string` would lose the
//! file's structure entirely).
//!
//! # Lock acquire order (load-bearing)
//!
//! CRUD handlers MUST hold [`ConfigWriter::lock`] across the entire
//! `write_atomically → reload → (rollback)` sequence. Without that,
//! two concurrent CRUD requests race on the disk file: req1 writes →
//! req2 overwrites → req1 invokes reload, sees req2's bytes, and
//! returns 201 to req1's client even though req1's POST never
//! landed.
//!
//! Story 9-7's `ConfigReloadHandle::reload` has its own internal
//! `tokio::sync::Mutex` (see `src/config_reload.rs:145`). The two
//! mutexes are independent and acquired in **consistent order**
//! (write_lock → reload mutex), so there is no deadlock risk: a
//! SIGHUP-triggered reload waits for the write_lock-holding CRUD
//! handler to release, but never the reverse.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tempfile::NamedTempFile;
use tokio::sync::{Mutex, MutexGuard};
use toml_edit::DocumentMut;

use crate::utils::OpcGwError;

/// Owns the canonical `config.toml` path + a write-lock that
/// serialises concurrent CRUD requests for the entire write+reload
/// critical section.
///
/// **Iter-1 review D3-P (poisoning):** the writer carries an
/// `AtomicBool poisoned` that is set when `rollback()` itself fails.
/// Future `load_document` / `write_atomically` / `rollback` calls
/// short-circuit on the poisoned flag with an `OpcGwError::Web`
/// describing the inconsistent state — the caller maps this to HTTP
/// 503 ("gateway in inconsistent state, restart required") so
/// operators can distinguish "transient IO" (500) from
/// "irrecoverable state" (503).
pub struct ConfigWriter {
    config_path: PathBuf,
    write_lock: Mutex<()>,
    poisoned: AtomicBool,
}

impl ConfigWriter {
    /// Construct from a config path. Story 9-4 review iter-1 P21:
    /// canonicalize the path so subsequent cwd changes do not drift
    /// the rename target away from figment's read target. Falls back
    /// to the original path if canonicalization fails (e.g., the
    /// file has not been created yet — rare, but happens in tests).
    pub fn new(config_path: PathBuf) -> Arc<Self> {
        let canonical = config_path
            .canonicalize()
            .unwrap_or_else(|_| config_path.clone());
        Arc::new(Self {
            config_path: canonical,
            write_lock: Mutex::new(()),
            poisoned: AtomicBool::new(false),
        })
    }

    /// Iter-1 review D3-P: caller-checkable poison flag.
    ///
    /// **Iter-2 review P31:** uses `Ordering::Acquire` so a sibling
    /// CRUD handler that just rolled back + set the flag with
    /// `Ordering::Release` is reliably observed here. Relaxed
    /// ordering would make the cross-thread visibility timing-
    /// dependent, defeating the iter-1 D3-P intent.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Acquire)
    }

    /// Iter-1 review D3-P: helper that returns the standard poisoned
    /// error so call sites stay terse + the wording is uniform.
    fn poisoned_err() -> OpcGwError {
        OpcGwError::Web(
            "config writer poisoned: a prior rollback IO failed; \
             gateway is in an inconsistent state. Restart required."
                .to_string(),
        )
    }

    /// Test-only accessor. Hidden behind `#[cfg(test)]` per iter-1
    /// review P29 — production code already holds `Arc<ConfigWriter>`
    /// and reads via `read_raw` / `load_document`.
    #[cfg(test)]
    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    /// Acquire the write-lock; the caller must hold the resulting
    /// guard across `write_atomically` AND the subsequent
    /// `ConfigReloadHandle::reload()` call AND any `rollback`. Drop
    /// the guard only after reload (or rollback) returns.
    pub async fn lock(&self) -> MutexGuard<'_, ()> {
        self.write_lock.lock().await
    }

    /// Read the TOML file from disk and parse it as a `DocumentMut`.
    /// Returns the document on success or
    /// [`OpcGwError::Web`](crate::utils::OpcGwError::Web) on IO or
    /// parse failure.
    ///
    /// **Iter-2 review P30:** the production CRUD handlers prefer
    /// [`Self::parse_document_from_bytes`] (the bytes returned by
    /// `read_raw()` are reused for both rollback snapshot AND
    /// document parsing — eliminates the TOCTOU window between two
    /// `std::fs::read_*` calls). `load_document` is retained for
    /// callers that don't need rollback (e.g., one-shot reads in
    /// tests).
    #[allow(dead_code)]
    pub fn load_document(&self) -> Result<DocumentMut, OpcGwError> {
        if self.is_poisoned() {
            return Err(Self::poisoned_err());
        }
        let bytes = std::fs::read_to_string(&self.config_path).map_err(|e| {
            OpcGwError::Web(format!(
                "failed to read config TOML for editing at {}: {e}",
                self.config_path.display()
            ))
        })?;
        bytes.parse::<DocumentMut>().map_err(|e| {
            OpcGwError::Web(format!(
                "failed to parse config TOML for editing at {}: {e}",
                self.config_path.display()
            ))
        })
    }

    /// Iter-2 review P30: parse a `DocumentMut` from already-read
    /// raw bytes. Use this AFTER `read_raw()` so the rollback
    /// snapshot and the document being mutated are guaranteed to
    /// represent the same on-disk state. Eliminates the TOCTOU
    /// window between two `std::fs::read_*` calls.
    pub fn parse_document_from_bytes(
        &self,
        bytes: &[u8],
    ) -> Result<DocumentMut, OpcGwError> {
        if self.is_poisoned() {
            return Err(Self::poisoned_err());
        }
        let s = std::str::from_utf8(bytes).map_err(|e| {
            OpcGwError::Web(format!("config TOML at {} is not valid UTF-8: {e}", self.config_path.display()))
        })?;
        s.parse::<DocumentMut>().map_err(|e| {
            OpcGwError::Web(format!(
                "failed to parse config TOML for editing at {}: {e}",
                self.config_path.display()
            ))
        })
    }

    /// Read the raw bytes of the TOML file. Used by CRUD handlers to
    /// snapshot the pre-write state for rollback. Returns
    /// [`OpcGwError::Web`](crate::utils::OpcGwError::Web) on IO
    /// failure.
    pub fn read_raw(&self) -> Result<Vec<u8>, OpcGwError> {
        if self.is_poisoned() {
            return Err(Self::poisoned_err());
        }
        std::fs::read(&self.config_path).map_err(|e| {
            OpcGwError::Web(format!(
                "failed to read config TOML at {} for rollback snapshot: {e}",
                self.config_path.display()
            ))
        })
    }

    /// Atomically write `bytes` to the canonical config path via
    /// `tempfile + rename` (POSIX-atomic on the same filesystem).
    ///
    /// **Caller MUST hold [`Self::lock`] across this call** to
    /// serialise concurrent CRUD requests.
    ///
    /// **Iter-1 review P4 (durability):** the temp file's data is
    /// `fsync`'d before persist, and the parent directory is
    /// `fsync`'d after persist, so the rename + content survive a
    /// power loss. Without these `sync_all` calls, a crash between
    /// persist and the next system sync could leave the rename lost
    /// OR the file empty.
    pub fn write_atomically(&self, bytes: &[u8]) -> Result<(), OpcGwError> {
        if self.is_poisoned() {
            return Err(Self::poisoned_err());
        }
        let parent = self.config_path.parent().ok_or_else(|| {
            OpcGwError::Web(format!(
                "config path {} has no parent directory; cannot atomically replace",
                self.config_path.display()
            ))
        })?;
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };

        let mut tmp = NamedTempFile::new_in(parent).map_err(|e| {
            OpcGwError::Web(format!(
                "failed to create temp file in {} for atomic config write: {e}",
                parent.display()
            ))
        })?;

        use std::io::Write;
        tmp.write_all(bytes).map_err(|e| {
            OpcGwError::Web(format!(
                "failed to write candidate config bytes to temp file: {e}"
            ))
        })?;
        tmp.flush().map_err(|e| {
            OpcGwError::Web(format!(
                "failed to flush candidate config bytes to temp file: {e}"
            ))
        })?;
        // Iter-1 review P4: fsync the file's data BEFORE persist so a
        // crash leaves either the old file intact or the new file
        // fully on-disk — never zero-length.
        tmp.as_file().sync_all().map_err(|e| {
            OpcGwError::Web(format!(
                "failed to fsync candidate config tempfile: {e}"
            ))
        })?;

        tmp.persist(&self.config_path).map_err(|e| {
            OpcGwError::Web(format!(
                "failed to atomically rename temp config file into place at {}: {}",
                self.config_path.display(),
                e.error
            ))
        })?;

        // Iter-1 review P4: fsync the parent directory AFTER persist
        // so the rename itself is durable. POSIX atomicity covers
        // crash-during-rename, but the dir entry update is only
        // durable after the dir's fsync.
        // Iter-2 review P32: differentiate platform-unsupported
        // (Windows lacks dir-fsync — accept silently) from real IO
        // errors (Linux EIO indicates real corruption — surface).
        if let Ok(dir) = std::fs::File::open(parent) {
            if let Err(e) = dir.sync_all() {
                if e.kind() != std::io::ErrorKind::Unsupported {
                    return Err(OpcGwError::Web(format!(
                        "parent directory fsync failed: {e}"
                    )));
                }
            }
        }

        Ok(())
    }

    /// Restore the pre-write bytes via the same atomic-rename path.
    /// Used on reload-failure to revert the on-disk TOML so the next
    /// startup or SIGHUP doesn't trip over a known-bad file. Returns
    /// `Ok(())` on a successful rollback;
    /// [`OpcGwError::Web`](crate::utils::OpcGwError::Web) on rollback
    /// failure (the writer is poisoned in that case — see
    /// [`is_poisoned`](Self::is_poisoned) — and the caller MUST emit
    /// a critical-severity audit event).
    ///
    /// **Caller MUST hold [`Self::lock`] across this call.**
    ///
    /// **Iter-2 review P27:** `rollback()` BYPASSES the poison
    /// check so a FIRST rollback can always attempt recovery. If
    /// the rollback succeeds, the poison flag is CLEARED — the
    /// writer has recovered to a known-good state. If the rollback
    /// fails, the poison flag is SET; the caller maps this to HTTP
    /// 503 in `io_error_response`. A future caller (or a re-attempt
    /// after operator intervention) can call `rollback()` again
    /// despite the poison flag — the bypass is intentional.
    pub fn rollback(&self, original_bytes: &[u8]) -> Result<(), OpcGwError> {
        match self.write_atomically_inner(original_bytes) {
            Ok(()) => {
                // Iter-2 review P27: successful recovery clears the
                // poison flag — the writer is back to a known-good
                // state. Subsequent CRUD requests can proceed.
                self.poisoned.store(false, Ordering::Release);
                Ok(())
            }
            Err(e) => {
                // Iter-1 review D3-P: poison the writer so subsequent
                // CRUD handlers short-circuit with 503 instead of
                // racing on a known-broken file. Iter-2 review P31:
                // Release ordering pairs with the Acquire load in
                // `is_poisoned`.
                self.poisoned.store(true, Ordering::Release);
                Err(e)
            }
        }
    }

    /// Internal `write_atomically` that does NOT check the poison
    /// flag — used by `rollback` to attempt the recovery write
    /// before deciding whether to poison.
    fn write_atomically_inner(&self, bytes: &[u8]) -> Result<(), OpcGwError> {
        let parent = self.config_path.parent().ok_or_else(|| {
            OpcGwError::Web(format!(
                "config path {} has no parent directory; cannot atomically replace",
                self.config_path.display()
            ))
        })?;
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };

        let mut tmp = NamedTempFile::new_in(parent).map_err(|e| {
            OpcGwError::Web(format!(
                "failed to create temp file in {} for atomic config write: {e}",
                parent.display()
            ))
        })?;
        use std::io::Write;
        tmp.write_all(bytes).map_err(|e| {
            OpcGwError::Web(format!(
                "failed to write candidate config bytes to temp file: {e}"
            ))
        })?;
        tmp.flush().map_err(|e| {
            OpcGwError::Web(format!(
                "failed to flush candidate config bytes to temp file: {e}"
            ))
        })?;
        tmp.as_file().sync_all().map_err(|e| {
            OpcGwError::Web(format!(
                "failed to fsync candidate config tempfile: {e}"
            ))
        })?;
        tmp.persist(&self.config_path).map_err(|e| {
            OpcGwError::Web(format!(
                "failed to atomically rename temp config file into place at {}: {}",
                self.config_path.display(),
                e.error
            ))
        })?;
        // Iter-2 review P32: differentiate platform-unsupported from
        // real IO errors (same shape as `write_atomically`).
        if let Ok(dir) = std::fs::File::open(parent) {
            if let Err(e) = dir.sync_all() {
                if e.kind() != std::io::ErrorKind::Unsupported {
                    return Err(OpcGwError::Web(format!(
                        "parent directory fsync failed: {e}"
                    )));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::TempDir;

    fn fixture_toml() -> &'static str {
        "# OPERATOR_COMMENT_MARKER\n\
         [global]\n\
         debug = true\n\
         \n\
         [[application]]\n\
         application_name = \"Building Sensors\"\n\
         application_id = \"app-1\"\n"
    }

    fn make_writer() -> (Arc<ConfigWriter>, TempDir) {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, fixture_toml()).expect("write fixture");
        (ConfigWriter::new(path), dir)
    }

    #[test]
    fn load_document_returns_documentmut_for_valid_toml() {
        let (writer, _dir) = make_writer();
        let doc = writer.load_document().expect("load_document");
        // Round-trip preserves the comment marker.
        assert!(doc.to_string().contains("OPERATOR_COMMENT_MARKER"));
        // Round-trip preserves the application table.
        assert!(doc.to_string().contains("application_name = \"Building Sensors\""));
    }

    #[test]
    fn write_atomically_preserves_comments() {
        let (writer, _dir) = make_writer();
        let mut doc = writer.load_document().expect("load_document");

        // Mutate the [global].debug field.
        doc["global"]["debug"] = toml_edit::value(false);

        writer
            .write_atomically(doc.to_string().as_bytes())
            .expect("write_atomically");

        // Re-read raw bytes and verify the comment marker is still there.
        let raw = std::fs::read_to_string(writer.config_path()).expect("re-read");
        assert!(
            raw.contains("OPERATOR_COMMENT_MARKER"),
            "comment marker lost: {raw}"
        );
        assert!(
            raw.contains("debug = false"),
            "mutation did not land: {raw}"
        );
        // Application block still present.
        assert!(
            raw.contains("application_name = \"Building Sensors\""),
            "application block clobbered: {raw}"
        );
    }

    #[test]
    fn rollback_restores_original_bytes() {
        let (writer, _dir) = make_writer();
        let original = writer.read_raw().expect("read_raw");

        // Mutate + write.
        let mut doc = writer.load_document().expect("load_document");
        doc["global"]["debug"] = toml_edit::value(false);
        writer
            .write_atomically(doc.to_string().as_bytes())
            .expect("write_atomically");

        // Rollback.
        writer.rollback(&original).expect("rollback");

        let restored = writer.read_raw().expect("read_raw post rollback");
        assert_eq!(restored, original, "rollback did not restore byte-equal");
    }

    /// Iter-1 review P18: deterministic serialisation check via an
    /// AtomicU32 counter. Previous timing-based assertion (`elapsed
    /// >= 150ms`) was flake-prone on slow CI. The counter approach
    /// asserts the load-bearing property directly: at no point are
    /// both tasks inside the critical section simultaneously.
    /// Iter-2 review P33: strengthened with explicit `entered`
    /// counter (asserts BOTH tasks reached the critical section, so
    /// the test cannot pass vacuously if one task fails to spawn)
    /// AND with `tokio::sync::Notify` to force the second task to
    /// attempt acquisition WHILE the first is inside the critical
    /// section (a no-op `Mutex` would otherwise pass on a single-
    /// core runtime where the tasks don't naturally overlap).
    #[tokio::test]
    async fn lock_serialises_concurrent_writers() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let (writer, _dir) = make_writer();
        let writer2 = writer.clone();
        let in_critical = std::sync::Arc::new(AtomicU32::new(0));
        let max_observed = std::sync::Arc::new(AtomicU32::new(0));
        let entered = std::sync::Arc::new(AtomicU32::new(0));
        // t1 holds the lock; t2 signals on this Notify when it's
        // about to attempt acquisition. t1 then sleeps briefly so
        // t2 has time to actually contend.
        let t2_attempting = std::sync::Arc::new(tokio::sync::Notify::new());

        let in1 = in_critical.clone();
        let max1 = max_observed.clone();
        let entered1 = entered.clone();
        let notify1 = t2_attempting.clone();
        let t1 = tokio::spawn(async move {
            let _g = writer.lock().await;
            entered1.fetch_add(1, Ordering::SeqCst);
            let n = in1.fetch_add(1, Ordering::SeqCst) + 1;
            max1.fetch_max(n, Ordering::SeqCst);
            // Wait for t2 to signal it's contending (or 100ms
            // timeout for safety on broken implementations).
            let _ = tokio::time::timeout(
                Duration::from_millis(100),
                notify1.notified(),
            )
            .await;
            // Sleep a touch more so t2's lock().await is actually
            // pending while we're inside the critical section.
            tokio::time::sleep(Duration::from_millis(20)).await;
            in1.fetch_sub(1, Ordering::SeqCst);
        });
        let in2 = in_critical.clone();
        let max2 = max_observed.clone();
        let entered2 = entered.clone();
        let notify2 = t2_attempting.clone();
        let t2 = tokio::spawn(async move {
            // Tiny delay so t1 acquires first.
            tokio::time::sleep(Duration::from_millis(5)).await;
            notify2.notify_one();
            let _g = writer2.lock().await;
            entered2.fetch_add(1, Ordering::SeqCst);
            let n = in2.fetch_add(1, Ordering::SeqCst) + 1;
            max2.fetch_max(n, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(10)).await;
            in2.fetch_sub(1, Ordering::SeqCst);
        });
        let _ = tokio::join!(t1, t2);
        assert_eq!(
            entered.load(Ordering::SeqCst),
            2,
            "BOTH tasks must reach the critical section; got {} (test passes vacuously otherwise)",
            entered.load(Ordering::SeqCst)
        );
        assert_eq!(
            max_observed.load(Ordering::SeqCst),
            1,
            "lock did NOT serialise — both tasks were inside the critical section at the same time"
        );
    }

    /// Iter-1 review D3-P (poison flag): once `rollback()` fails,
    /// future `load_document` / `write_atomically` calls short-
    /// circuit with the poisoned error.
    #[tokio::test]
    async fn poisoned_writer_rejects_subsequent_writes() {
        // Seed a writer pointing at a tempdir, then force a rollback
        // failure by deleting the tempdir's parent before rollback.
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, fixture_toml()).expect("write fixture");
        let writer = ConfigWriter::new(path.clone());
        assert!(!writer.is_poisoned());

        // Drop the directory to make rollback fail (parent missing
        // → tempfile creation in the parent fails).
        let original_bytes = writer.read_raw().expect("read_raw");
        drop(dir); // <-- removes path's parent
        let rb = writer.rollback(&original_bytes);
        assert!(rb.is_err(), "rollback should fail when parent is gone");
        assert!(
            writer.is_poisoned(),
            "writer must be poisoned after rollback failure"
        );

        // Subsequent writes are refused.
        let next = writer.write_atomically(b"anything");
        assert!(next.is_err());
        let msg = next.unwrap_err().to_string();
        assert!(msg.contains("poisoned"), "expected poisoned error: {msg}");
    }
}

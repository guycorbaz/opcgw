// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! OPC UA security hardening helpers (Story 7-2).
//!
//! Three responsibilities live here so `config.rs` and `opc_ua.rs` don't
//! sprawl further:
//!
//! 1. [`validate_private_key_permissions`] — enforces NFR9: the OPC UA
//!    private-key file must have mode `0o600`. Called from
//!    [`crate::config::AppConfig::validate`] so misconfigured deployments
//!    fail fast at startup with an actionable error rather than silently
//!    running with a world-readable key.
//! 2. [`ensure_pki_directories`] — enforces FR45: the PKI directory layout
//!    (`own/`, `private/`, `trusted/`, `rejected/`) is verified at startup
//!    and missing directories are auto-created with the correct modes
//!    (`private/` → `0o700`, others → `0o755`). Called from
//!    [`crate::opc_ua::OpcUa::create_server`] before async-opcua's
//!    `ServerBuilder::pki_dir`.
//! 3. [`warn_if_create_sample_keypair_in_release`] — pure helper that
//!    returns a warning string when `create_sample_keypair = true` AND the
//!    binary is a release build. Wired from `main.rs`. The check is
//!    intentionally non-blocking — operators legitimately running
//!    release-mode dev builds should be allowed, just loud.

use std::path::Path;

use crate::utils::OpcGwError;

/// Validate that the OPC UA private-key file has mode `0o600` (NFR9).
///
/// # Behaviour
///
/// - If the file does not exist:
///   - and `create_sample_keypair == true`: returns `Ok(())`. async-opcua
///     will create it; permission setting is the operator's concern only
///     after a real keypair is provisioned. (Note: async-opcua 0.17.1
///     itself does NOT chmod the file to `0o600` after auto-creation —
///     this is documented in `docs/security.md` "Upgrading from Story 7-1"
///     and is the operator's responsibility on the next restart.)
///   - and `create_sample_keypair == false`: returns `Err`. A production
///     deployment with no keypair and auto-generation disabled cannot
///     start.
/// - If the file exists, reads its mode via [`std::os::unix::fs::MetadataExt`]
///   and rejects anything other than `0o600`. The error message includes
///   both the observed mode and the `chmod` recipe so operators don't have
///   to look up the syntax.
/// - On non-Unix platforms the permission check is skipped with a
///   `tracing::warn!`. The OPC UA gateway has no Windows deployment
///   contract today; if that changes the helper should be promoted to a
///   real cross-platform check.
///
/// # Returns
///
/// - `Ok(())` if the file is at `0o600`, the platform is non-Unix, or the
///   file is missing and `create_sample_keypair == true`.
/// - `Err(message)` for any other condition. The caller (`AppConfig::validate`)
///   is expected to push the message into its accumulating error vector so
///   all configuration violations surface in a single startup error.
pub fn validate_private_key_permissions(
    pki_dir: &str,
    private_key_path: &str,
    create_sample_keypair: bool,
) -> Result<(), String> {
    let absolute = Path::new(pki_dir).join(private_key_path);

    if !absolute.exists() {
        if create_sample_keypair {
            return Ok(());
        }
        return Err(format!(
            "opcua.private_key_path: file {} does not exist and create_sample_keypair is false. \
             Provision the keypair manually or set create_sample_keypair = true (development only).",
            absolute.display()
        ));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(&absolute).map_err(|e| {
            format!(
                "opcua.private_key_path: cannot read metadata for {}: {}",
                absolute.display(),
                e
            )
        })?;
        let mode = meta.mode() & 0o777;
        if mode != 0o600 {
            return Err(format!(
                "opcua.private_key_path: file {} has permissions 0o{:o}, must be 0o600 (NFR9). \
                 Run: chmod 600 {}",
                absolute.display(),
                mode,
                absolute.display()
            ));
        }
    }

    #[cfg(not(unix))]
    {
        tracing::warn!(
            event = "private_key_perm_check_skipped",
            path = %absolute.display(),
            "Skipping private-key permission check on non-Unix platform"
        );
    }

    Ok(())
}

/// Ensure the OPC UA PKI directory layout exists with correct modes (FR45).
///
/// Creates `<pki_dir>/{own,private,trusted,rejected}` if any are missing,
/// then on Unix sets the modes:
///
/// - `private/` → `0o700` (owner read/write/exec only — closes the "is the
///   key present?" side channel that `0o755` would leave open).
/// - `own/`, `trusted/`, `rejected/` → `0o755` (operators may need to drop
///   client certs into `trusted/` or inspect rejected ones).
///
/// On non-Unix platforms the chmod step is skipped (the directories are
/// still created).
///
/// Emits one `tracing::info!` event per directory that is created or
/// chmod'd, with `event = "pki_dir_initialised"`. Idempotent — re-running
/// on an already-correct layout is a no-op (no info events).
///
/// # Returns
///
/// - `Ok(())` on success.
/// - `Err(OpcGwError::Configuration)` if any `create_dir_all` or `set_mode`
///   call fails. The error message names the offending path and includes
///   the underlying I/O error.
pub fn ensure_pki_directories(pki_dir: &str) -> Result<(), OpcGwError> {
    const SUBDIRS: &[(&str, u32)] = &[
        ("own", 0o755),
        ("private", 0o700),
        ("trusted", 0o755),
        ("rejected", 0o755),
    ];

    for (name, expected_mode) in SUBDIRS {
        let path = Path::new(pki_dir).join(name);
        let existed = path.exists();
        if !existed {
            std::fs::create_dir_all(&path).map_err(|e| {
                OpcGwError::Configuration(format!(
                    "Failed to ensure PKI directory layout under {}: cannot create {}: {}",
                    pki_dir,
                    path.display(),
                    e
                ))
            })?;
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(&path).map_err(|e| {
                OpcGwError::Configuration(format!(
                    "Failed to ensure PKI directory layout under {}: cannot stat {}: {}",
                    pki_dir,
                    path.display(),
                    e
                ))
            })?;
            let actual_mode = meta.permissions().mode() & 0o777;
            if actual_mode != *expected_mode {
                let mut perms = meta.permissions();
                perms.set_mode(*expected_mode);
                std::fs::set_permissions(&path, perms).map_err(|e| {
                    OpcGwError::Configuration(format!(
                        "Failed to ensure PKI directory layout under {}: cannot chmod {} to 0o{:o}: {}",
                        pki_dir,
                        path.display(),
                        expected_mode,
                        e
                    ))
                })?;
                tracing::info!(
                    event = "pki_dir_initialised",
                    path = %path.display(),
                    created = !existed,
                    mode_set = format!("0o{:o}", expected_mode),
                    "Initialised PKI directory"
                );
            } else if !existed {
                tracing::info!(
                    event = "pki_dir_initialised",
                    path = %path.display(),
                    created = true,
                    mode_set = format!("0o{:o}", expected_mode),
                    "Initialised PKI directory"
                );
            }
        }

        #[cfg(not(unix))]
        {
            if !existed {
                tracing::info!(
                    event = "pki_dir_initialised",
                    path = %path.display(),
                    created = true,
                    mode_set = "skipped (non-Unix)",
                    "Initialised PKI directory (mode skipped on non-Unix)"
                );
            }
        }
    }

    Ok(())
}

/// Pure helper for the AC#6 release-build warning (Story 7-2).
///
/// Returns `Some(warning text)` when `create_sample_keypair` is `true`
/// AND the binary is built in release mode. Returns `None` otherwise.
///
/// Factored out as a pure function purely for testability — the
/// `cfg!(debug_assertions)` flag is evaluated at the call site (`main.rs`)
/// and passed in as `is_release`.
pub fn warn_if_create_sample_keypair_in_release(
    create_sample_keypair: bool,
    is_release: bool,
) -> Option<String> {
    if create_sample_keypair && is_release {
        Some(
            "opcua.create_sample_keypair = true in a release build. \
             Set create_sample_keypair = false and provision keypair manually for production \
             deployments. See docs/security.md."
                .to_string(),
        )
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;

    fn tmp_dir(prefix: &str) -> std::path::PathBuf {
        // Use a UUID-based temp dir to avoid collisions under parallel
        // `cargo test`. `tempfile` is a dev-dep added by Story 7-2 (Task 6)
        // and is preferred for new tests, but at this layer we only need a
        // path — std::env::temp_dir() + uuid keeps the dependency surface
        // narrow and is sufficient for a unit test.
        let dir = std::env::temp_dir()
            .join(format!("opcgw_sec_{}_{}", prefix, uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    // ---------------------------------------------------------------------
    // validate_private_key_permissions (AC#4)
    // ---------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn test_validation_rejects_world_readable_private_key() {
        let dir = tmp_dir("validate_perm_reject");
        let private_dir = dir.join("private");
        fs::create_dir_all(&private_dir).expect("create private/");
        let key_path = private_dir.join("private.pem");
        let _ = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o644)
            .open(&key_path)
            .expect("create key file");

        let err = validate_private_key_permissions(
            dir.to_str().unwrap(),
            "private/private.pem",
            true, // even with sample-keypair=true, an existing file must be 0600
        )
        .expect_err("must reject 0o644 key file");

        assert!(
            err.contains("0o644"),
            "error must include observed mode 0o644, got: {err}"
        );
        assert!(
            err.contains("chmod 600"),
            "error must include the chmod recipe, got: {err}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn test_validation_accepts_0600_private_key() {
        let dir = tmp_dir("validate_perm_accept");
        let private_dir = dir.join("private");
        fs::create_dir_all(&private_dir).expect("create private/");
        let key_path = private_dir.join("private.pem");
        let _ = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&key_path)
            .expect("create key file");

        validate_private_key_permissions(
            dir.to_str().unwrap(),
            "private/private.pem",
            false,
        )
        .expect("0o600 file must be accepted");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_validation_skips_permission_check_when_create_sample_keypair_true_and_file_missing() {
        let dir = tmp_dir("validate_perm_skip_missing");
        // No file at all under private/.
        validate_private_key_permissions(
            dir.to_str().unwrap(),
            "private/private.pem",
            true,
        )
        .expect("missing file with create_sample_keypair=true must be Ok");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_validation_rejects_missing_key_when_create_sample_keypair_false() {
        let dir = tmp_dir("validate_perm_missing_no_create");
        let err = validate_private_key_permissions(
            dir.to_str().unwrap(),
            "private/private.pem",
            false,
        )
        .expect_err("missing file with create_sample_keypair=false must be Err");

        assert!(
            err.contains("does not exist"),
            "error must mention missing file, got: {err}"
        );
        assert!(
            err.contains("create_sample_keypair"),
            "error must mention create_sample_keypair, got: {err}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    // ---------------------------------------------------------------------
    // ensure_pki_directories (AC#5)
    // ---------------------------------------------------------------------

    #[test]
    fn test_ensure_pki_directories_creates_all_four() {
        let dir = tmp_dir("pki_create_all");
        ensure_pki_directories(dir.to_str().unwrap()).expect("ensure_pki_directories");

        for sub in ["own", "private", "trusted", "rejected"] {
            assert!(
                dir.join(sub).is_dir(),
                "expected {sub} subdirectory under {}",
                dir.display()
            );
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let private_mode = fs::metadata(dir.join("private"))
                .expect("stat private/")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(
                private_mode, 0o700,
                "private/ must be 0o700, got 0o{:o}",
                private_mode
            );

            for sub in ["own", "trusted", "rejected"] {
                let m = fs::metadata(dir.join(sub))
                    .expect("stat sub")
                    .permissions()
                    .mode()
                    & 0o777;
                assert_eq!(m, 0o755, "{sub}/ must be 0o755, got 0o{:o}", m);
            }
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_ensure_pki_directories_idempotent() {
        let dir = tmp_dir("pki_idempotent");
        ensure_pki_directories(dir.to_str().unwrap()).expect("first call");
        // Second call must succeed without error.
        ensure_pki_directories(dir.to_str().unwrap()).expect("second call (idempotent)");

        // Modes must still be correct.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = fs::metadata(dir.join("private"))
                .expect("stat")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(m, 0o700);
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn test_ensure_pki_directories_fixes_loose_private_dir_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp_dir("pki_fix_loose");
        let private_dir = dir.join("private");
        fs::create_dir_all(&private_dir).expect("create private/");
        // Pre-create with too-loose mode.
        let mut perms = fs::metadata(&private_dir)
            .expect("stat")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&private_dir, perms).expect("chmod 0755");

        ensure_pki_directories(dir.to_str().unwrap()).expect("ensure_pki_directories");

        let new_mode = fs::metadata(&private_dir)
            .expect("stat after")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            new_mode, 0o700,
            "ensure_pki_directories must tighten private/ from 0o755 → 0o700, got 0o{:o}",
            new_mode
        );

        let _ = fs::remove_dir_all(&dir);
    }

    // ---------------------------------------------------------------------
    // warn_if_create_sample_keypair_in_release (AC#6)
    // ---------------------------------------------------------------------

    #[test]
    fn test_warn_when_create_true_and_release_true() {
        let warn = warn_if_create_sample_keypair_in_release(true, true);
        assert!(warn.is_some(), "must warn for (create=true, release=true)");
        let msg = warn.unwrap();
        assert!(
            msg.contains("create_sample_keypair = true"),
            "warning must mention the flag, got: {msg}"
        );
        assert!(
            msg.contains("docs/security.md"),
            "warning must point at docs, got: {msg}"
        );
    }

    #[test]
    fn test_no_warn_when_create_false_and_release_true() {
        assert_eq!(
            warn_if_create_sample_keypair_in_release(false, true),
            None,
            "no warning expected for (create=false, release=true)"
        );
    }

    #[test]
    fn test_no_warn_when_create_true_and_release_false() {
        assert_eq!(
            warn_if_create_sample_keypair_in_release(true, false),
            None,
            "no warning expected in dev/debug builds"
        );
    }

    #[test]
    fn test_no_warn_when_create_false_and_release_false() {
        assert_eq!(
            warn_if_create_sample_keypair_in_release(false, false),
            None,
            "no warning expected for (create=false, release=false)"
        );
    }
}

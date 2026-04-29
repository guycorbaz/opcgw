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

use std::path::{Component, Path};

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
) -> Result<(), OpcGwError> {
    if pki_dir.trim().is_empty() {
        return Err(OpcGwError::Configuration(
            "opcua.pki_dir: must not be empty or whitespace-only (relative paths are resolved \
             against the gateway's current working directory; an empty value silently falls \
             back to cwd)"
                .to_string(),
        ));
    }

    let private_path_typed = Path::new(private_key_path);
    if private_path_typed.is_absolute() {
        return Err(OpcGwError::Configuration(format!(
            "opcua.private_key_path: must be a path relative to opcua.pki_dir, got absolute path \
             {private_key_path:?}. An absolute path silently escapes the configured PKI directory."
        )));
    }
    if private_path_typed
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(OpcGwError::Configuration(format!(
            "opcua.private_key_path: must not contain `..` components, got {private_key_path:?}. \
             Such paths can silently escape the configured PKI directory."
        )));
    }

    let absolute = Path::new(pki_dir).join(private_key_path);

    if !absolute.exists() {
        if create_sample_keypair {
            return Ok(());
        }
        return Err(OpcGwError::Configuration(format!(
            "opcua.private_key_path: file {} does not exist and create_sample_keypair is false. \
             Provision the keypair manually or set create_sample_keypair = true (development only).",
            absolute.display()
        )));
    }

    #[cfg(unix)]
    {
        // N4: accumulate file-mode AND parent-mode violations so an
        // operator who has both wrong sees both reported in a single
        // restart, rather than fixing the file mode, restarting, then
        // having to fix the parent mode and restart again.
        use std::os::unix::fs::MetadataExt;
        let mut violations: Vec<String> = Vec::new();

        let meta = std::fs::metadata(&absolute).map_err(|e| {
            OpcGwError::Configuration(format!(
                "opcua.private_key_path: cannot read metadata for {}: {}",
                absolute.display(),
                e
            ))
        })?;
        let mode = meta.mode() & 0o777;
        if mode != 0o600 {
            violations.push(format!(
                "opcua.private_key_path: file {} has permissions 0o{:o}, must be 0o600 (NFR9). \
                 Run: chmod 600 {}",
                absolute.display(),
                mode,
                absolute.display()
            ));
        }

        // Defence-in-depth: a 0o600 file under a 0o755 directory is still
        // discoverable (other users can `ls` and see filenames). If the
        // parent dir exists, it must be 0o700. If it does not exist yet,
        // `ensure_pki_directories` will create it correctly later.
        if let Some(parent) = absolute.parent() {
            if parent.exists() {
                let parent_meta = std::fs::metadata(parent).map_err(|e| {
                    OpcGwError::Configuration(format!(
                        "opcua.private_key_path: cannot read metadata for parent {}: {}",
                        parent.display(),
                        e
                    ))
                })?;
                let parent_mode = parent_meta.mode() & 0o777;
                if parent_mode != 0o700 {
                    violations.push(format!(
                        "opcua.private_key_path: parent directory {} has permissions 0o{:o}, \
                         must be 0o700 (NFR9). Run: chmod 700 {}",
                        parent.display(),
                        parent_mode,
                        parent.display()
                    ));
                }
            }
        }

        if !violations.is_empty() {
            return Err(OpcGwError::Configuration(violations.join(" | ")));
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

    if pki_dir.trim().is_empty() {
        return Err(OpcGwError::Configuration(
            "opcua.pki_dir: must not be empty or whitespace-only when initialising PKI directory layout"
                .to_string(),
        ));
    }

    for (name, expected_mode) in SUBDIRS {
        let path = Path::new(pki_dir).join(name);

        // P9: a regular file (or symlink to a non-directory) at the PKI
        // subdir path is a misconfiguration we must not silently chmod into
        // a 0o700 file. Detect and error early.
        if path.exists() && !path.is_dir() {
            return Err(OpcGwError::Configuration(format!(
                "PKI path {} exists but is not a directory; refusing to overwrite or chmod a non-directory entry",
                path.display()
            )));
        }

        let existed = path.is_dir();
        if !existed {
            // P3: race-free create with the target mode atomic to creation
            // (Unix) so the directory is born `0o700` rather than created
            // 0o755 then chmodded — no world-readable window.
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(*expected_mode)
                    .create(&path)
                    .map_err(|e| {
                        OpcGwError::Configuration(format!(
                            "Failed to ensure PKI directory layout under {}: cannot create {}: {}",
                            pki_dir,
                            path.display(),
                            e
                        ))
                    })?;
            }
            #[cfg(not(unix))]
            {
                std::fs::create_dir_all(&path).map_err(|e| {
                    OpcGwError::Configuration(format!(
                        "Failed to ensure PKI directory layout under {}: cannot create {}: {}",
                        pki_dir,
                        path.display(),
                        e
                    ))
                })?;
            }
            tracing::info!(
                event = "pki_dir_initialised",
                path = %path.display(),
                created = true,
                mode_set = format!("0o{:o}", expected_mode),
                "Initialised PKI directory"
            );
            continue;
        }

        // Pre-existing directory: verify and tighten mode if needed.
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
                    created = false,
                    mode_set = format!("0o{:o}", expected_mode),
                    "Tightened PKI directory mode"
                );
            }
            // Idempotent silence when the directory already had the correct mode.
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
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp_dir("validate_perm_reject");
        let private_dir = dir.join("private");
        fs::create_dir_all(&private_dir).expect("create private/");
        // Tighten parent dir to 0o700 so the parent-mode check (P20) doesn't
        // pre-empt the file-mode check we want to exercise here.
        let mut parent_perms = fs::metadata(&private_dir).expect("stat parent").permissions();
        parent_perms.set_mode(0o700);
        fs::set_permissions(&private_dir, parent_perms).expect("chmod 0700 parent");

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

        let msg = err.to_string();
        assert!(
            msg.contains("0o644"),
            "error must include observed mode 0o644, got: {msg}"
        );
        assert!(
            msg.contains("chmod 600"),
            "error must include the chmod recipe, got: {msg}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn test_validation_accepts_0600_private_key() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp_dir("validate_perm_accept");
        let private_dir = dir.join("private");
        fs::create_dir_all(&private_dir).expect("create private/");
        // Tighten parent dir to 0o700 so the parent-mode check (P20) passes.
        let mut parent_perms = fs::metadata(&private_dir).expect("stat parent").permissions();
        parent_perms.set_mode(0o700);
        fs::set_permissions(&private_dir, parent_perms).expect("chmod 0700 parent");

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

    #[cfg(unix)]
    #[test]
    fn test_validation_rejects_loose_parent_dir_mode() {
        // P20: a 0o600 key file under a non-0o700 parent is still
        // discoverable; the validator must reject the parent dir mode too.
        // Force the parent to 0o755 explicitly so the test is independent
        // of the host's umask (which can yield 0o755 or 0o775 depending
        // on configuration).
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp_dir("validate_loose_parent");
        let private_dir = dir.join("private");
        fs::create_dir_all(&private_dir).expect("create private/");
        let mut parent_perms = fs::metadata(&private_dir).expect("stat parent").permissions();
        parent_perms.set_mode(0o755);
        fs::set_permissions(&private_dir, parent_perms).expect("chmod 0755 parent");

        let key_path = private_dir.join("private.pem");
        let _ = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&key_path)
            .expect("create key file");

        let err = validate_private_key_permissions(
            dir.to_str().unwrap(),
            "private/private.pem",
            false,
        )
        .expect_err("0o755 parent dir must be rejected even with 0o600 file");
        let msg = err.to_string();
        assert!(
            msg.contains("parent directory"),
            "error must mention parent directory, got: {msg}"
        );
        assert!(
            msg.contains("0o755") && msg.contains("0o700"),
            "error must include observed (0o755) and required (0o700) mode, got: {msg}"
        );

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

        let msg = err.to_string();
        assert!(
            msg.contains("does not exist"),
            "error must mention missing file, got: {msg}"
        );
        assert!(
            msg.contains("create_sample_keypair"),
            "error must mention create_sample_keypair, got: {msg}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_validation_rejects_empty_pki_dir() {
        // P2: empty pki_dir silently falls back to cwd; reject explicitly.
        let err = validate_private_key_permissions("", "private/private.pem", true)
            .expect_err("empty pki_dir must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("pki_dir") && msg.contains("must not be empty"),
            "error must mention empty pki_dir, got: {msg}"
        );
    }

    #[test]
    fn test_validation_rejects_absolute_private_key_path() {
        // P1: absolute paths discard pki_dir via Path::join; reject explicitly.
        let err = validate_private_key_permissions("/var/lib/pki", "/etc/shadow", false)
            .expect_err("absolute private_key_path must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("absolute path"),
            "error must mention absolute path traversal, got: {msg}"
        );
    }

    #[test]
    fn test_validation_rejects_dotdot_in_private_key_path() {
        // P1: `..` components silently escape pki_dir; reject explicitly.
        let err = validate_private_key_permissions(
            "/var/lib/pki",
            "../../../etc/shadow",
            false,
        )
        .expect_err("`..` in private_key_path must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("`..`") || msg.contains(".."),
            "error must mention `..` traversal, got: {msg}"
        );
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

    #[test]
    fn test_ensure_pki_directories_rejects_regular_file_at_subdir_path() {
        // P9: a regular file at <pki_dir>/private must be rejected, not
        // silently chmodded into a 0o700 file.
        let dir = tmp_dir("pki_file_collision");
        let private_path = dir.join("private");
        fs::write(&private_path, b"not a directory").expect("write file");

        let err = ensure_pki_directories(dir.to_str().unwrap())
            .expect_err("regular file at private/ must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("not a directory"),
            "error must mention non-directory, got: {msg}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_ensure_pki_directories_rejects_empty_pki_dir() {
        // P2: empty pki_dir must be rejected here too.
        let err = ensure_pki_directories("")
            .expect_err("empty pki_dir must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("pki_dir") && msg.contains("must not be empty"),
            "error must mention empty pki_dir, got: {msg}"
        );
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

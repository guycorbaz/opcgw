// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Shared HMAC-SHA-256 keyed credential digest (Story 9-1, extracted from
//! Story 7-2's `OpcgwAuthManager`).
//!
//! Both auth surfaces in the gateway — the OPC UA `OpcgwAuthManager`
//! (Story 7-2) and the embedded web-server `WebAuthState` (Story 9-1) —
//! compute HMAC-SHA-256 digests of the configured credentials under a
//! per-process random key, then constant-time compare submitted-credential
//! digests against the stored ones. The keying makes the digest
//! non-deterministic across processes (so a digest cannot be replayed
//! against a different gateway instance) and the SHA-256 output is
//! fixed-length, eliminating the length oracle that a direct content
//! compare would leave open.
//!
//! This module exposes the primitive function so both call sites share
//! one implementation. Per the Phase-B carry-forward rule
//! (`epics.md:782`): reuse, don't roll new.

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Compute `HMAC-SHA-256(key, data)` and return the 32-byte digest.
///
/// Used by [`OpcgwAuthManager`](crate::opc_ua_auth::OpcgwAuthManager) and
/// [`WebAuthState`](crate::web::auth::WebAuthState) to hash both
/// configured and submitted credentials before constant-time comparison.
/// The HMAC keying makes the digest non-deterministic across processes
/// (so a digest cannot be replayed against a different gateway instance)
/// and the SHA-256 output is fixed-length, eliminating the length oracle
/// that a direct content compare would leave open.
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    // E6: `Hmac::<Sha256>::new_from_slice` accepts arbitrary key lengths
    // (it is the variable-key constructor — the fixed-key path is
    // `new`/`new_from_slice` on a `Mac`-trait impl with a `KeySize`
    // type-level constant, which we do not use). The `expect` is
    // therefore unreachable for SHA-256-keyed HMAC; calling it out here
    // so future readers don't grep for "InvalidLength" handling.
    let mut mac = HmacSha256::new_from_slice(key)
        .expect("Hmac::new_from_slice never fails for variable-key HMAC");
    mac.update(data);
    let result = mac.finalize().into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same key + same input must produce the same digest. Pinning the
    /// determinism of the primitive — without it, the auth comparison
    /// would never match.
    #[test]
    fn hmac_sha256_is_deterministic_under_same_key() {
        let key = [0x42u8; 32];
        let a = hmac_sha256(&key, b"opcua-user");
        let b = hmac_sha256(&key, b"opcua-user");
        assert_eq!(a, b, "same key + same input must produce same digest");
    }

    /// Different inputs under the same key must produce different digests
    /// (collision resistance is delegated to SHA-256; this test only
    /// pins that the primitive does not silently strip or hash to a
    /// constant).
    #[test]
    fn hmac_sha256_differs_for_different_inputs() {
        let key = [0x42u8; 32];
        let a = hmac_sha256(&key, b"opcua-user");
        let b = hmac_sha256(&key, b"opcua-other");
        assert_ne!(a, b, "different inputs must produce different digests");
    }

    /// Different per-process keys must produce different digests for the
    /// same input — so digests cannot be replayed across gateway
    /// instances.
    #[test]
    fn hmac_sha256_differs_for_different_keys() {
        let a = hmac_sha256(&[0x42u8; 32], b"opcua-user");
        let b = hmac_sha256(&[0x77u8; 32], b"opcua-user");
        assert_ne!(
            a, b,
            "different per-process keys must produce different digests \
             (so digests cannot be replayed across gateway instances)"
        );
    }

    /// Closes the length oracle: regardless of input length the digest
    /// is always 32 bytes, so `constant_time_eq` over digests can no
    /// longer short-circuit.
    #[test]
    fn hmac_sha256_output_is_fixed_length_for_any_input() {
        let key = [0u8; 32];
        let short = hmac_sha256(&key, b"a");
        let medium = hmac_sha256(&key, b"opcua-user");
        let long = hmac_sha256(&key, &[0x55u8; 4096]);
        assert_eq!(short.len(), 32);
        assert_eq!(medium.len(), 32);
        assert_eq!(long.len(), 32);
    }
}

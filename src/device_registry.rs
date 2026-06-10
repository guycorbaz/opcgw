// SPDX-License-Identifier: MIT OR Apache-2.0
// (c) [2026] Guy Corbaz

//! Device-class + per-model adapter registry (Epic E, Story E-2).
//!
//! A *class* defines a canonical OPC UA command/status surface; a per-class
//! [`DeviceDriver`] translates a canonical OPC UA command value into the
//! ChirpStack downlink form. Increment 1 (#135) exposed the `command_class`
//! binding through the web command surface; this increment extracts the
//! concrete valve mapping — previously inline in
//! [`crate::chirpstack::map_command_to_downlink`] — behind the [`DeviceDriver`]
//! trait so further classes/models are an additive registration rather than a
//! new `match` arm.
//!
//! ## Tiers (per the 2026-06-09 E-2 redesign)
//!
//! A driver chooses, independently per direction, how it translates:
//!
//! - **T1 codec-canonical** — the editable ChirpStack codec already emits/accepts
//!   opcgw's canonical shape, so opcgw does no transform. The Tonhe valve is T1:
//!   its decode is a passthrough (the codec produces the canonical
//!   `valveStatusCode` / `valvePosition` / … fields, which the uplink-ingestion
//!   path in [`crate::chirpstack_events`] stores as-is), so only the downlink
//!   *encode* is opcgw's concern — implemented by [`ValveDriver`] here.
//! - **T2 vendor-object remap** / **T3 native bytes** — owned by opcgw when the
//!   codec cannot be edited to the canonical shape. The object-remap transform
//!   engine (enum / scale+offset / bitmask) and a second driver land in a later
//!   increment; that is also when a `decode_uplink` hook is added to this trait.

use crate::chirpstack::{valve_command_object, DownlinkPayload};
use crate::utils::OpcGwError;
use std::sync::LazyLock;

/// Translates a canonical OPC UA command value into a ChirpStack downlink for
/// one device class.
///
/// Uplink decode is a passthrough for Tier-1 classes (the codec already emits
/// canonical fields), so the trait carries only the downlink `encode_command`
/// for now; a `decode_uplink` hook is added with the Tier-2 remap engine.
pub(crate) trait DeviceDriver: Send + Sync {
    /// The `command_class` string this driver is registered under.
    fn class_name(&self) -> &'static str;

    /// Encode a canonical command payload — the raw OPC UA command-node value,
    /// already narrowed to bytes by `OpcUaServer::set_command` — into a
    /// [`DownlinkPayload`]. A bad value is returned as an error so it is visible
    /// rather than silently mis-sent.
    fn encode_command(&self, raw_payload: &[u8]) -> Result<DownlinkPayload, OpcGwError>;
}

/// Tier-1 driver for the Tonhe E20/A20 motorized valve (and any valve whose
/// ChirpStack codec speaks the canonical `{"command":"open"|"close"}` object).
///
/// A valve command is a single canonical byte: `1` = open, `0` = close. The
/// codec's `encodeDownlink` turns the semantic object into the model's wire
/// bytes (Tonhe: fPort 10, `0x01` / `0x02`), keeping opcgw model-agnostic.
pub(crate) struct ValveDriver;

impl DeviceDriver for ValveDriver {
    fn class_name(&self) -> &'static str {
        "valve"
    }

    fn encode_command(&self, raw_payload: &[u8]) -> Result<DownlinkPayload, OpcGwError> {
        // Reject empty or multi-byte payloads rather than silently truncating
        // to the first byte.
        if raw_payload.len() != 1 {
            return Err(OpcGwError::ChirpStack(format!(
                "valve command expects a 1-byte payload, got {} byte(s)",
                raw_payload.len()
            )));
        }
        let command = match raw_payload[0] {
            1 => "open",
            0 => "close",
            other => {
                return Err(OpcGwError::ChirpStack(format!(
                    "valve command value {} out of range (expected 1=open or 0=close)",
                    other
                )))
            }
        };
        Ok(DownlinkPayload::Object(valve_command_object(command)))
    }
}

/// Registry of the compiled-in device-class drivers, resolved by
/// `command_class` string.
pub(crate) struct ClassRegistry {
    drivers: Vec<Box<dyn DeviceDriver>>,
}

impl ClassRegistry {
    /// Construct the registry with every built-in driver.
    fn with_builtin_drivers() -> Self {
        Self {
            drivers: vec![Box::new(ValveDriver)],
        }
    }

    /// Resolve a `command_class` string to its driver, if one is registered.
    pub(crate) fn driver_for(&self, class: &str) -> Option<&dyn DeviceDriver> {
        self.drivers
            .iter()
            .find(|d| d.class_name() == class)
            .map(|d| d.as_ref())
    }

    /// All registered class names — the single source of truth for "what is a
    /// valid `command_class`", used by the web CRUD validator and
    /// `AppConfig::validate` so neither maintains a separate hardcoded list.
    pub(crate) fn class_names(&self) -> Vec<&'static str> {
        self.drivers.iter().map(|d| d.class_name()).collect()
    }
}

/// Process-wide registry. The built-in driver set is static (compiled in), so a
/// single lazily-initialised instance is shared by all command deliveries.
static REGISTRY: LazyLock<ClassRegistry> = LazyLock::new(ClassRegistry::with_builtin_drivers);

/// The shared device-class registry.
pub(crate) fn registry() -> &'static ClassRegistry {
    &REGISTRY
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_resolves_valve_and_rejects_unknown() {
        let reg = registry();
        assert!(reg.driver_for("valve").is_some(), "valve must be registered");
        assert!(
            reg.driver_for("sprocket").is_none(),
            "an unregistered class must resolve to None"
        );
    }

    #[test]
    fn valve_driver_encodes_open_and_close() {
        let d = ValveDriver;
        let open = d.encode_command(&[1]).expect("open encodes");
        let close = d.encode_command(&[0]).expect("close encodes");
        // Both must produce a semantic object (not raw bytes)...
        assert!(matches!(open, DownlinkPayload::Object(_)), "1 must encode an Object");
        assert!(matches!(close, DownlinkPayload::Object(_)), "0 must encode an Object");
        // ...carrying the correct, DISTINCT command verb. Asserting open
        // contains "open" but not "close" (and vice-versa) catches a swapped
        // or identical mapping — the failure mode a Debug-substring-only guard
        // on a single direction would miss.
        let open_s = format!("{open:?}");
        let close_s = format!("{close:?}");
        assert!(
            open_s.contains("open") && !open_s.contains("close"),
            "1 must encode open exactly, got {open_s}"
        );
        assert!(
            close_s.contains("close") && !close_s.contains("open"),
            "0 must encode close exactly, got {close_s}"
        );
    }

    #[test]
    fn valve_driver_rejects_bad_payloads() {
        let d = ValveDriver;
        assert!(d.encode_command(&[]).is_err(), "empty payload rejected");
        assert!(d.encode_command(&[1, 2]).is_err(), "multi-byte rejected");
        assert!(d.encode_command(&[5]).is_err(), "out-of-range value rejected");
    }
}

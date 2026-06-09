-- SPDX-License-Identifier: MIT OR Apache-2.0
-- (c) [2026] Guy Corbaz
--
-- Migration v012 — Story E-1 (E-1b, #132): per-device OPC UA stale threshold.
--
-- Adds an optional per-device stale threshold (seconds) that overrides the
-- global [opcua].stale_threshold_seconds for that device only. NULL = use the
-- global default. Lets slow LoRaWAN sensors (~15-20 min cadence) read Good
-- instead of Uncertain between uplinks without loosening the global threshold.
ALTER TABLE devices ADD COLUMN stale_threshold_seconds INTEGER;

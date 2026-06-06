-- SPDX-License-Identifier: MIT OR Apache-2.0
-- Copyright (c) [2026] Guy Corbaz
--
-- Story E-0 (Epic E): device-class binding for commands
--
-- Adds an optional `command_class` to the application-config `commands` table
-- so a command can be bound to a device class (e.g. "valve"). When set, the
-- runtime poller translates the canonical OPC UA command value into a semantic
-- command object that the ChirpStack device-profile codec encodes into wire
-- bytes (keeping opcgw model-agnostic). NULL = the legacy raw-byte downlink
-- path (unchanged behaviour for every existing command).
--
-- Nullable with no default: existing rows read back as command_class = NULL,
-- which maps to the raw-byte fallback — a backward-compatible no-op upgrade.

ALTER TABLE commands ADD COLUMN command_class TEXT;

-- Story C-6: Application configuration tables
--
-- Promotes the [[application]] tree from TOML to SQLite as the
-- authoritative store for applications, devices, metrics, and commands.
-- CASCADE on FK delete mirrors the nested-TOML semantics: removing an
-- application removes all its devices, metrics, and commands.
--
-- Composite PKs enforce C-3's duplicate-prevention at the schema level
-- (defence-in-depth on top of the server-side validator).

CREATE TABLE IF NOT EXISTS applications (
    application_id   TEXT NOT NULL,
    application_name TEXT NOT NULL,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL,
    PRIMARY KEY (application_id)
);

CREATE TABLE IF NOT EXISTS devices (
    application_id TEXT NOT NULL,
    device_id      TEXT NOT NULL,
    device_name    TEXT NOT NULL,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL,
    PRIMARY KEY (application_id, device_id),
    FOREIGN KEY (application_id) REFERENCES applications(application_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS metrics (
    application_id           TEXT NOT NULL,
    device_id                TEXT NOT NULL,
    chirpstack_metric_name   TEXT NOT NULL,
    metric_name              TEXT NOT NULL,
    metric_type              TEXT NOT NULL,
    metric_unit              TEXT,
    created_at               TEXT NOT NULL,
    updated_at               TEXT NOT NULL,
    PRIMARY KEY (application_id, device_id, chirpstack_metric_name),
    FOREIGN KEY (application_id, device_id)
        REFERENCES devices(application_id, device_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS commands (
    application_id   TEXT NOT NULL,
    device_id        TEXT NOT NULL,
    command_name     TEXT NOT NULL,
    command_id       INTEGER NOT NULL,
    command_confirmed INTEGER NOT NULL DEFAULT 0,
    command_port      INTEGER,
    PRIMARY KEY (application_id, device_id, command_name),
    FOREIGN KEY (application_id, device_id)
        REFERENCES devices(application_id, device_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_devices_by_app
    ON devices(application_id);

CREATE INDEX IF NOT EXISTS idx_metrics_by_device
    ON metrics(application_id, device_id);

CREATE INDEX IF NOT EXISTS idx_commands_by_device
    ON commands(application_id, device_id);

-- Migration metadata: persists the C-6 done-flag so the idempotency guard
-- survives operator-driven deletions of all applications via the web UI.
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT NOT NULL PRIMARY KEY,
    value TEXT NOT NULL
);

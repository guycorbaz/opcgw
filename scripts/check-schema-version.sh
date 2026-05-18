#!/bin/sh
# scripts/check-schema-version.sh
#
# Story A-7 — operator pre-flight check for the Epic A migration.
#
# Reads `PRAGMA user_version` from an opcgw SQLite database and prints
# the current schema version plus a one-line recommendation about which
# migration path applies. See `docs/deployment-guide.md § "Epic A migration"`
# for the full runbook.
#
# Usage:
#   ./scripts/check-schema-version.sh <path-to-opcgw.db>
#
# Exit codes:
#   0 — success (database file exists, version read, recommendation printed)
#   1 — database file not found, not openable, or unrecognised schema version
#   2 — invocation error (missing argument, sqlite3 CLI not installed)
#
# Portability: POSIX `/bin/sh` (works under bash, dash, busybox ash). Story
# A-7 iter-1 K5 review fix converted from bash to POSIX so the script runs
# on Alpine and other distros where /bin/sh is not bash. No `[[ ]]`, no
# `pipefail`; just `[ ]` and `set -eu`.
#
# Dependency: `sqlite3` CLI (universally available on operator-class Linux
# distros: `apt-get install sqlite3` / `yum install sqlite` / `apk add sqlite`).

set -eu

if [ $# -ne 1 ]; then
  printf 'Usage: %s <path-to-opcgw.db>\n' "$0" >&2
  printf '\n' >&2
  printf 'Reads PRAGMA user_version from the opcgw SQLite database and recommends\n' >&2
  printf 'the appropriate Epic A migration path. See docs/deployment-guide.md.\n' >&2
  exit 2
fi

DB="$1"

if ! command -v sqlite3 >/dev/null 2>&1; then
  printf 'ERROR: sqlite3 CLI not found in PATH.\n' >&2
  printf 'Install via your distro package manager:\n' >&2
  printf '  Debian/Ubuntu: apt-get install sqlite3\n' >&2
  printf '  RHEL/CentOS:   yum install sqlite\n' >&2
  printf '  Alpine:        apk add sqlite\n' >&2
  exit 2
fi

if [ ! -f "$DB" ]; then
  printf 'ERROR: database file not found: %s\n' "$DB" >&2
  printf 'If this is a fresh deployment with no existing database, no migration\n' >&2
  printf 'is needed — the gateway will create a v008 database on first startup.\n' >&2
  exit 1
fi

# Read the schema version. `PRAGMA user_version` always returns an integer
# (default 0 for fresh databases), so a sqlite3 failure here means the file
# isn't a valid SQLite database.
#
# iter-2 L8 review fix: strip trailing whitespace / CR-LF via `tr -d` so
# the case-statement match doesn't fall through to `*)` on platforms where
# sqlite3 emits trailing newlines or carriage returns.
if ! VERSION_RAW="$(sqlite3 "$DB" 'PRAGMA user_version;' 2>/dev/null)"; then
  printf 'ERROR: cannot read schema version from %s\n' "$DB" >&2
  printf 'The file exists but does not appear to be a valid SQLite database.\n' >&2
  exit 1
fi
VERSION="$(printf '%s' "$VERSION_RAW" | tr -d '[:space:]')"

# iter-2 L7 review fix: verify the database is actually an opcgw database
# (has both `metric_values` and `metric_history` tables). Without this
# check, pointing the script at any unrelated SQLite file (Firefox
# places.sqlite, Chrome history, etc. — all of which have user_version=0)
# would land in the pre-Epic-A recommendation arm and tell the operator
# to back up + run the v2.0 gateway against the wrong file.
#
# Skip the table check for the `8` case (post-migration verification can
# run against a database whose schema is already complete — the table
# check passes there too — so this is mostly forward-defensive).
if ! TABLE_COUNT="$(sqlite3 "$DB" "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('metric_values', 'metric_history');" 2>/dev/null)"; then
  printf 'ERROR: cannot query sqlite_master on %s\n' "$DB" >&2
  exit 1
fi
TABLE_COUNT="$(printf '%s' "$TABLE_COUNT" | tr -d '[:space:]')"
if [ "$TABLE_COUNT" != "2" ]; then
  printf 'ERROR: %s does not appear to be an opcgw database (missing metric_values and/or metric_history tables).\n' "$DB" >&2
  printf 'Found %s of the 2 required tables. Are you pointing the script at the right file?\n' "$TABLE_COUNT" >&2
  exit 1
fi

printf 'Database:               %s\n' "$DB"
printf 'Current schema version: %s\n' "$VERSION"

case "$VERSION" in
  0|1|2|3|4|5|6)
    printf 'Status:                 pre-Epic-A (v2.0-rc baseline)\n'
    printf '\n'
    printf 'Recommendation:\n'
    printf "  Path A (preserve historical rows): take a file-level backup of '%s',\n" "$DB"
    printf '    then start the v2.0 gateway against the same file. The v007 + v008\n'
    printf '    migrations apply automatically. Pre-Epic-A rows tag as value_type='\''legacy'\''\n'
    printf '    and surface as BadDataUnavailable in OPC UA until the next poll cycle\n'
    printf '    UPSERTs a typed payload.\n'
    printf "  Path B (drop-and-recreate): rm '%s' '%s-wal' '%s-shm' before starting the v2.0\n" "$DB" "$DB" "$DB"
    printf '    gateway. A fresh v008 database is created on startup.\n'
    printf '\n'
    printf 'See docs/deployment-guide.md § "Epic A migration" for the full runbook.\n'
    ;;
  7)
    printf 'Status:                 partial Epic A (v007 applied, v008 pending)\n'
    printf '\n'
    printf 'Recommendation:\n'
    printf '  An interrupted prior upgrade left the database between v007 and v008\n'
    printf '  (v007 commits independently before v008 starts — see runbook § "Epic A\n'
    printf '  migration"). Start the v2.0 gateway to complete the migration —\n'
    printf '  run_migrations is idempotent and will apply v008 only.\n'
    ;;
  8)
    printf 'Status:                 Epic A complete (v008)\n'
    printf '\n'
    printf 'Recommendation:\n'
    printf '  No migration needed. The database is already at the latest Epic A schema.\n'
    ;;
  *)
    printf 'Status:                 UNRECOGNISED schema version\n' >&2
    printf '\n' >&2
    printf 'WARNING: version %s is higher than the latest known schema (8).\n' "$VERSION" >&2
    printf 'Are you running an old binary against a newer database? Check that the\n' >&2
    printf 'binary version matches the database. Aborting without recommendation.\n' >&2
    exit 1
    ;;
esac

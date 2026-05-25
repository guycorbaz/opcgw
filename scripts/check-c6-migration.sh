#!/usr/bin/env bash
# check-c6-migration.sh — Post-C-6 migration verification tool.
#
# Usage: bash scripts/check-c6-migration.sh [opcgw.db]
#
# Queries the SQLite database at the given path (default: opcgw.db in the
# current directory) and prints a green/red summary of the C-6 migration
# status.
#
# Exit codes:
#   0  — migration completed and counts look valid
#   1  — migration not yet run, or counts are zero
#   2  — sqlite3 not found or DB not readable

set -euo pipefail

DB="${1:-opcgw.db}"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
NC='\033[0m' # No Colour

fail() { echo -e "${RED}FAIL${NC}: $1"; exit "${2:-1}"; }
pass() { echo -e "${GREEN}PASS${NC}: $1"; }
info() { echo -e "${YELLOW}INFO${NC}: $1"; }

# ------------------------------------------------------------------
# 0. Prerequisites
# ------------------------------------------------------------------
if ! command -v sqlite3 >/dev/null 2>&1; then
    fail "sqlite3 not found. Install the sqlite3 CLI to run this check." 2
fi

if [[ ! -r "$DB" ]]; then
    fail "Cannot read database at '$DB'. Pass the correct path as the first argument." 2
fi

echo "Checking C-6 migration status for: $DB"
echo "---------------------------------------"

# ------------------------------------------------------------------
# 1. Schema version
# ------------------------------------------------------------------
# opcgw tracks schema version via PRAGMA user_version (not a meta table).
SCHEMA_VER=$(sqlite3 "$DB" "PRAGMA user_version;" 2>/dev/null || echo "0")

if [[ "$SCHEMA_VER" -ge 9 ]] 2>/dev/null; then
    pass "Schema version: $SCHEMA_VER (>= 9, migration schema applied)"
else
    fail "Schema version is '$SCHEMA_VER' — migration schema v009 has not been applied. Upgrade the binary and restart opcgw." 1
fi

# ------------------------------------------------------------------
# 2. Migration done-flag
# ------------------------------------------------------------------
# Written by migrate_applications_config() inside the EXCLUSIVE TRANSACTION.
# Key is 'c6_migration_done'; value is the ISO-8601 timestamp of migration.
MIG_TS=$(sqlite3 "$DB" "SELECT value FROM meta WHERE key='c6_migration_done';" 2>/dev/null || echo "")
if [[ -n "$MIG_TS" ]]; then
    pass "Migration done-flag: $MIG_TS"
else
    info "No migration done-flag found. Migration may not have run yet, or the gateway was started fresh (C-0 empty-bootstrap path)."
fi

# ------------------------------------------------------------------
# 3. Row counts
# ------------------------------------------------------------------
APP_COUNT=$(sqlite3 "$DB" "SELECT COUNT(*) FROM applications;" 2>/dev/null || echo "-1")
DEV_COUNT=$(sqlite3 "$DB" "SELECT COUNT(*) FROM devices;" 2>/dev/null || echo "-1")
MET_COUNT=$(sqlite3 "$DB" "SELECT COUNT(*) FROM metrics;" 2>/dev/null || echo "-1")
CMD_COUNT=$(sqlite3 "$DB" "SELECT COUNT(*) FROM commands;" 2>/dev/null || echo "-1")

echo ""
echo "Row counts:"
echo "  applications : $APP_COUNT"
echo "  devices      : $DEV_COUNT"
echo "  metrics      : $MET_COUNT"
echo "  commands     : $CMD_COUNT"
echo ""

if [[ "$APP_COUNT" -lt 0 ]]; then
    fail "Could not query tables — schema v009 may not have been applied." 1
fi

if [[ "$APP_COUNT" -eq 0 && -z "$MIG_TS" ]]; then
    info "No applications in SQLite and no migration done-flag. The gateway has likely not started yet, or is running in C-0 empty-bootstrap mode."
    exit 0
fi

if [[ "$APP_COUNT" -eq 0 && -n "$MIG_TS" ]]; then
    info "Migration done-flag is set ($MIG_TS) but applications table is empty — operator has deleted all applications via the web UI (normal post-migration state)."
    exit 0
fi

# ------------------------------------------------------------------
# 4. Sample rows
# ------------------------------------------------------------------
echo "Sample applications:"
sqlite3 "$DB" "SELECT application_id, application_name FROM applications LIMIT 5;" 2>/dev/null \
    | sed 's/^/  /'
echo ""

# ------------------------------------------------------------------
# 5. Summary
# ------------------------------------------------------------------
pass "C-6 migration check complete. $APP_COUNT application(s), $DEV_COUNT device(s), $MET_COUNT metric(s), $CMD_COUNT command(s)."
exit 0
